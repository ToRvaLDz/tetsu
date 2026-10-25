//! Initrd service: seal the root volume to the TPM at the first boot with
//! Secure Boot on. Never fails the boot: errors are printed and the exit
//! status stays 0. See `tetsu_installer_core::seal`.

use std::process::ExitCode;

use tetsu_installer_core::seal::{self, Outcome, SealPaths};
use tetsu_tx::boot::tool::SystemRunner;

fn main() -> ExitCode {
    match seal::run(&SealPaths::system(), &SystemRunner) {
        Ok(Outcome::NotNeeded) => {}
        Ok(Outcome::Sealed) => println!("tetsu-unlock-seal: the disk is now sealed to the TPM"),
        Ok(Outcome::Waiting(why)) => {
            println!("tetsu-unlock-seal: unlock level stays pending: {why}");
        }
        Err(e) => eprintln!("tetsu-unlock-seal: not sealed, will try again at the next boot: {e}"),
    }
    ExitCode::SUCCESS
}
