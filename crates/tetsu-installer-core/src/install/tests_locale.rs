//! `GenerateLocale` on the fake machine: a language the payload lacks.

use std::fs;
use std::path::PathBuf;

use crate::preflight::SecureBoot;
use crate::unlock::UnlockLevel;

use super::fake::{Outcome, answers, run_plan, run_with, target};
use super::plan::{InstallPlan, Stage};

const NEVER: &dyn Fn() -> bool = &|| false;
const PAYLOAD_LOCALE_GEN: &str = "it_IT.UTF-8 UTF-8\nen_US.UTF-8 UTF-8\n";

struct Run {
    out: Outcome,
    root: tempfile::TempDir,
}

impl Run {
    fn work(&self, rel: &str) -> PathBuf {
        self.root.path().join("work").join(rel)
    }

    fn etc(&self, name: &str) -> String {
        fs::read_to_string(self.work(&format!("top/@snapshots/1/root/etc/{name}"))).unwrap()
    }

    fn nspawn(&self) -> Vec<String> {
        let lines = self.out.machine.lines();
        lines
            .into_iter()
            .filter(|l| l.starts_with("systemd-nspawn"))
            .collect()
    }

    fn error(&self) -> String {
        self.out.result.as_ref().unwrap_err().to_string()
    }
}

fn install(language: &str, setup: impl FnOnce(&super::fake::Machine)) -> Run {
    let root = tempfile::tempdir().unwrap();
    let mut a = answers(UnlockLevel::Comfort);
    a.locale.language = language.into();
    let t = target(root.path(), SecureBoot::SetupMode);
    let full = InstallPlan::build(&a, &t);
    let plan = InstallPlan::from_actions(full.stage(Stage::Disk).collect());
    let out = run_with(&a, &t, &plan, NEVER, setup);
    Run { out, root }
}

#[test]
fn another_language_is_generated_in_the_target() {
    let run = install("es_MX.UTF-8", |_| {});
    run.out.result.as_ref().unwrap();
    let locale_gen = run.etc("locale.gen");
    assert_eq!(locale_gen, format!("{PAYLOAD_LOCALE_GEN}es_MX.UTF-8 UTF-8\n"));
    assert_eq!(run.etc("locale.conf"), "LANG=es_MX.UTF-8\n");

    let root = format!("-D {}", run.work("top/@snapshots/1/root").display());
    let nspawn = run.nspawn();
    assert_eq!(nspawn.len(), 5, "{nspawn:#?}");
    assert!(nspawn[0].ends_with("locale-gen"), "{}", nspawn[0]);
    assert!(
        nspawn[1].ends_with("localedef --list-archive"),
        "{}",
        nspawn[1]
    );
    assert!(nspawn[2].ends_with("pacman-key --init"));
    assert!(nspawn[3].contains("--populate"));
    assert!(nspawn[4].contains("mkinitcpio"));
    for l in &nspawn[..2] {
        assert!(l.contains(&root) && l.contains("--private-network"), "{l}");
        assert!(l.contains("locale-var:/var"), "{l}");
    }
}

#[test]
fn a_codesetless_entry_is_written_as_glibc_lists_it() {
    let run = install("aa_ER.UTF-8", |_| {});
    run.out.result.as_ref().unwrap();
    assert!(run.etc("locale.gen").ends_with("\naa_ER UTF-8\n"));
}

#[test]
fn a_language_the_targets_glibc_lacks_stops_before_anything_is_written() {
    let run = install("xx_YY.UTF-8", |_| {});
    assert!(run.error().contains("SUPPORTED"), "{}", run.error());
    assert_eq!(run.etc("locale.gen"), PAYLOAD_LOCALE_GEN);
    let lines = run.out.machine.lines();
    assert!(!lines.iter().any(|l| l.contains("locale-gen")));
    assert!(!lines.iter().any(|l| l.contains("mkinitcpio")));
}

#[test]
fn a_locale_that_was_not_archived_fails_the_install() {
    let run = install("es_MX.UTF-8", |m| m.no_locale_archive.set(true));
    assert!(run.error().contains("es_MX.utf8"), "{}", run.error());
    let lines = run.out.machine.lines();
    assert!(!lines.iter().any(|l| l.contains("mkinitcpio")));
}

#[test]
fn a_language_with_a_newline_never_reaches_locale_conf() {
    let run = install("it_IT.UTF-8\nLANG=x", |_| {});
    assert!(run.error().contains("invalid language"), "{}", run.error());
    let conf = run.work("top/@snapshots/1/root/etc/locale.conf");
    assert!(!conf.exists());
}

#[test]
fn a_pregenerated_language_runs_no_locale_gen() {
    let root = tempfile::tempdir().unwrap();
    let a = answers(UnlockLevel::Comfort);
    let t = target(root.path(), SecureBoot::SetupMode);
    let full = InstallPlan::build(&a, &t);
    let plan = InstallPlan::from_actions(full.stage(Stage::Disk).collect());
    let out = run_plan(&a, &t, &plan, NEVER);
    out.result.as_ref().unwrap();
    assert!(!out.machine.lines().iter().any(|l| l.contains("locale-gen")));
}
