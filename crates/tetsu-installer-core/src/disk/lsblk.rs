//! Parse `lsblk --json` output and the `/dev/disk/by-id` links.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::{Disk, Partition};
use crate::error::InstallError;

/// Columns requested from lsblk; sizes in bytes.
pub const LSBLK_COLUMNS: &str = "NAME,PATH,SIZE,MODEL,TYPE,RM,PTTYPE,FSTYPE,LABEL";

pub fn lsblk_args() -> Vec<OsString> {
    ["--json", "-b", "-o", LSBLK_COLUMNS]
        .into_iter()
        .map(OsString::from)
        .collect()
}

/// Old lsblk prints numbers and booleans as strings.
fn flexible_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flexible_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_u64() == Some(1),
        Value::String(s) => matches!(s.trim(), "1" | "true"),
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
struct Output {
    blockdevices: Vec<Node>,
}

#[derive(Debug, Deserialize)]
struct Node {
    name: String,
    path: Option<String>,
    #[serde(default)]
    size: Value,
    model: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    rm: Value,
    fstype: Option<String>,
    label: Option<String>,
    #[serde(default)]
    children: Vec<Node>,
}

impl Node {
    fn device_path(&self) -> PathBuf {
        self.path
            .as_deref()
            .map_or_else(|| Path::new("/dev").join(&self.name), PathBuf::from)
    }

    fn partition(&self) -> Partition {
        Partition {
            path: self.device_path(),
            size: flexible_u64(&self.size).unwrap_or(0),
            fstype: self.fstype.clone().filter(|s| !s.is_empty()),
            label: self.label.clone().filter(|s| !s.is_empty()),
        }
    }

    /// Partitions below this node, however deeply nested.
    fn partitions(&self) -> Vec<Partition> {
        let mut out = Vec::new();
        for child in &self.children {
            if child.kind.as_deref() == Some("part") {
                out.push(child.partition());
            }
            out.extend(child.partitions());
        }
        out
    }

    /// Every device path below this node.
    fn descendants(&self) -> Vec<PathBuf> {
        self.children
            .iter()
            .flat_map(|c| std::iter::once(c.device_path()).chain(c.descendants()))
            .collect()
    }
}

/// Whole disks from lsblk's JSON, with their partitions. `by_id` is left
/// empty here. Each disk is paired with the device paths below it.
pub fn parse(json: &str) -> Result<Vec<(Disk, Vec<PathBuf>)>, InstallError> {
    let out: Output = serde_json::from_str(json)?;
    Ok(out
        .blockdevices
        .iter()
        .filter(|n| n.kind.as_deref() == Some("disk"))
        .map(|n| {
            let disk = Disk {
                name: n.name.clone(),
                path: n.device_path(),
                by_id: None,
                size: flexible_u64(&n.size).unwrap_or(0),
                model: n
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned),
                removable: flexible_bool(&n.rm),
                partitions: n.partitions(),
                os: None,
            };
            (disk, n.descendants())
        })
        .collect())
}

/// Stable `/dev/disk/by-id` links of whole disks, keyed by kernel name.
/// Partition links and `wwn-` aliases lose to descriptive names.
pub fn by_id_links(root: &Path) -> HashMap<String, PathBuf> {
    let dir = root.join("dev/disk/by-id");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return HashMap::new();
    };
    let mut links: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            let target = std::fs::read_link(e.path()).ok()?;
            let name = target.file_name()?.to_str()?.to_owned();
            Some((id, name))
        })
        .filter(|(id, _)| !id.contains("-part"))
        .collect();
    // Descriptive ids first, then alphabetical, so the pick is stable.
    links.sort_by(|a, b| {
        let rank = |id: &str| u8::from(id.starts_with("wwn-") || id.starts_with("nvme-eui."));
        rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0))
    });
    let mut out = HashMap::new();
    for (id, name) in links {
        out.entry(name).or_insert_with(|| dir.join(id));
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"{
  "blockdevices": [
    {"name":"sda","path":"/dev/sda","size":512110190592,"model":"Samsung SSD 860 ","type":"disk","rm":false,"pttype":"gpt","fstype":null,"label":null,
     "children":[
       {"name":"sda1","path":"/dev/sda1","size":104857600,"model":null,"type":"part","rm":false,"pttype":"gpt","fstype":"vfat","label":"SYSTEM"},
       {"name":"sda2","path":"/dev/sda2","size":511000000000,"model":null,"type":"part","rm":false,"pttype":"gpt","fstype":"crypto_LUKS","label":null,
        "children":[{"name":"root","path":"/dev/mapper/root","size":510000000000,"model":null,"type":"crypt","rm":false,"pttype":null,"fstype":"ext4","label":"Arch"}]}
     ]},
    {"name":"sr0","path":"/dev/sr0","size":1073741312,"model":"DVD","type":"rom","rm":true,"pttype":null,"fstype":"iso9660","label":"ISO"},
    {"name":"sdb","path":"/dev/sdb","size":"15609618432","model":"USB Stick","type":"disk","rm":"1","pttype":"dos","fstype":null,"label":null}
  ]
}"#;

    #[test]
    fn parses_disks_with_children_and_skips_other_types() {
        let disks = parse(SAMPLE).unwrap();
        assert_eq!(disks.len(), 2);
        let (sda, below) = &disks[0];
        assert_eq!(sda.name, "sda");
        assert_eq!(sda.model.as_deref(), Some("Samsung SSD 860"));
        assert_eq!(sda.size, 512_110_190_592);
        assert!(!sda.removable);
        assert_eq!(sda.partitions.len(), 2);
        assert_eq!(sda.partitions[0].fstype.as_deref(), Some("vfat"));
        assert!(below.contains(&PathBuf::from("/dev/mapper/root")));
    }

    #[test]
    fn accepts_string_numbers_and_booleans() {
        let disks = parse(SAMPLE).unwrap();
        let (sdb, _) = &disks[1];
        assert_eq!(sdb.size, 15_609_618_432);
        assert!(sdb.removable);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(parse("nope"), Err(InstallError::Lsblk(_))));
    }

    #[test]
    fn args_ask_for_bytes_and_json() {
        let args = lsblk_args();
        assert_eq!(args[0], "--json");
        assert_eq!(args[1], "-b");
        assert_eq!(args[3], LSBLK_COLUMNS);
    }

    #[test]
    fn by_id_prefers_descriptive_names_and_skips_partitions() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("dev/disk/by-id");
        std::fs::create_dir_all(&dir).unwrap();
        let link = |id: &str, to: &str| std::os::unix::fs::symlink(to, dir.join(id)).unwrap();
        link("wwn-0x5002538e", "../../sda");
        link("ata-Samsung_SSD_860_S3Z9", "../../sda");
        link("ata-Samsung_SSD_860_S3Z9-part1", "../../sda1");
        link("usb-Gen_USB_0001", "../../sdb");
        let map = by_id_links(t.path());
        assert_eq!(map["sda"], dir.join("ata-Samsung_SSD_860_S3Z9"));
        assert_eq!(map["sdb"], dir.join("usb-Gen_USB_0001"));
        assert!(!map.contains_key("sda1"));
    }

    #[test]
    fn missing_by_id_directory_is_empty() {
        let t = tempfile::tempdir().unwrap();
        assert!(by_id_links(t.path()).is_empty());
    }
}
