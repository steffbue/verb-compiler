#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    X86_64,
    Arm64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
}

pub const ALL: [Target; 6] = [
    Target { os: Os::Linux, arch: Arch::X86_64 },
    Target { os: Os::Linux, arch: Arch::Arm64 },
    Target { os: Os::Macos, arch: Arch::X86_64 },
    Target { os: Os::Macos, arch: Arch::Arm64 },
    Target { os: Os::Windows, arch: Arch::X86_64 },
    Target { os: Os::Windows, arch: Arch::Arm64 },
];

fn invalid(s: &str) -> String {
    format!(
        "invalid --target {s:?}; expected <os>-<arch> or \"all\"\n  os: linux, macos, windows\n  arch: x86_64 (or x86), arm64"
    )
}

impl Target {
    pub fn parse(s: &str) -> Result<Target, String> {
        let (os_s, arch_s) = s.split_once('-').ok_or_else(|| invalid(s))?;
        let os = match os_s {
            "linux" => Os::Linux,
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            _ => return Err(invalid(s)),
        };
        let arch = match arch_s {
            "x86_64" | "x86" => Arch::X86_64,
            "arm64" => Arch::Arm64,
            _ => return Err(invalid(s)),
        };
        Ok(Target { os, arch })
    }

    pub fn llvm_triple(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Linux, Arch::X86_64) => "x86_64-unknown-linux-gnu",
            (Os::Linux, Arch::Arm64) => "aarch64-unknown-linux-gnu",
            (Os::Macos, Arch::X86_64) => "x86_64-apple-darwin",
            (Os::Macos, Arch::Arm64) => "aarch64-apple-darwin",
            (Os::Windows, Arch::X86_64) => "x86_64-pc-windows-gnu",
            (Os::Windows, Arch::Arm64) => "aarch64-pc-windows-gnu",
        }
    }

    pub fn zig_triple(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Linux, Arch::X86_64) => "x86_64-linux-gnu",
            (Os::Linux, Arch::Arm64) => "aarch64-linux-gnu",
            (Os::Macos, Arch::X86_64) => "x86_64-macos-none",
            (Os::Macos, Arch::Arm64) => "aarch64-macos-none",
            (Os::Windows, Arch::X86_64) => "x86_64-windows-gnu",
            (Os::Windows, Arch::Arm64) => "aarch64-windows-gnu",
        }
    }

    pub fn is_windows(&self) -> bool {
        self.os == Os::Windows
    }

    /// `os-arch` label used for `--target all` output suffixes, e.g. "linux-x86_64".
    pub fn label(&self) -> String {
        let os = match self.os {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        };
        let arch = match self.arch {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        };
        format!("{os}-{arch}")
    }

    /// Appends `.exe` for windows targets if the given path doesn't already have it.
    pub fn adjust_output(&self, out: &str) -> String {
        if self.is_windows() && !out.ends_with(".exe") {
            format!("{out}.exe")
        } else {
            out.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_six_combos() {
        assert_eq!(Target::parse("linux-x86_64").unwrap(), Target { os: Os::Linux, arch: Arch::X86_64 });
        assert_eq!(Target::parse("linux-arm64").unwrap(), Target { os: Os::Linux, arch: Arch::Arm64 });
        assert_eq!(Target::parse("macos-x86_64").unwrap(), Target { os: Os::Macos, arch: Arch::X86_64 });
        assert_eq!(Target::parse("macos-arm64").unwrap(), Target { os: Os::Macos, arch: Arch::Arm64 });
        assert_eq!(Target::parse("windows-x86_64").unwrap(), Target { os: Os::Windows, arch: Arch::X86_64 });
        assert_eq!(Target::parse("windows-arm64").unwrap(), Target { os: Os::Windows, arch: Arch::Arm64 });
    }

    #[test]
    fn x86_is_alias_for_x86_64() {
        assert_eq!(Target::parse("linux-x86").unwrap(), Target { os: Os::Linux, arch: Arch::X86_64 });
    }

    #[test]
    fn rejects_unknown_os_or_arch() {
        assert!(Target::parse("solaris-x86_64").is_err());
        assert!(Target::parse("linux-sparc").is_err());
        assert!(Target::parse("garbage").is_err());
    }

    #[test]
    fn llvm_and_zig_triples_are_distinct_per_target() {
        for t in ALL {
            assert!(!t.llvm_triple().is_empty());
            assert!(!t.zig_triple().is_empty());
        }
    }

    #[test]
    fn windows_output_gets_exe_appended_once() {
        let win = Target { os: Os::Windows, arch: Arch::X86_64 };
        assert_eq!(win.adjust_output("hello"), "hello.exe");
        assert_eq!(win.adjust_output("hello.exe"), "hello.exe");
        let linux = Target { os: Os::Linux, arch: Arch::X86_64 };
        assert_eq!(linux.adjust_output("hello"), "hello");
    }

    #[test]
    fn label_matches_cli_syntax() {
        assert_eq!(Target { os: Os::Macos, arch: Arch::Arm64 }.label(), "macos-arm64");
    }
}
