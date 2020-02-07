#![deny(warnings, unused_must_use)]
#![feature(thread_id_value)]

extern crate log;

use linux_loader::*;
use rcore_fs_hostfs::HostFS;
use std::io::Write;
use zircon_object::object::*;

fn main() {
    init_logger();
    kernel_hal_unix::init();

    let args: Vec<_> = std::env::args().skip(1).collect();
    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/x86_64-alpine-linux-musl/bin".into()];

    let exec_path = args[0].clone();
    let hostfs = HostFS::new("rootfs");
    let proc = run(&exec_path, args, envs, hostfs);
    proc.wait_signal(Signal::PROCESS_TERMINATED);
}

fn init_logger() {
    env_logger::builder()
        .format(|buf, record| {
            let tid = std::thread::current().id().as_u64();
            writeln!(buf, "[{:>5}][{}] {}", record.level(), tid, record.args())
        })
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test(cmdline: &str) {
        kernel_hal_unix::init();

        let args: Vec<String> = cmdline.split(' ').map(|s| s.into()).collect();
        let envs = vec![]; // TODO
        let exec_path = args[0].clone();
        let hostfs = HostFS::new("../rootfs");
        let proc = run(&exec_path, args, envs, hostfs);
        proc.wait_signal(Signal::PROCESS_TERMINATED);
    }

    #[test]
    fn busybox() {
        test("/bin/busybox");
    }
}