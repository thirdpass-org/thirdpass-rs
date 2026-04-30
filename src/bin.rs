use vouch_lib::extension::FromLib;
use vouch_rs_lib;

fn main() {
    let mut extension = vouch_rs_lib::RsExtension::new();
    vouch_lib::extension::commands::run(&mut extension).unwrap();
}
