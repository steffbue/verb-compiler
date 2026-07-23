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

    /// Best-effort match of an LLVM host triple (from
    /// `TargetMachine::get_default_triple()`) against one of the six supported
    /// targets, so `verb targets` can mark the host. Returns `None` for a host
    /// whose os/arch isn't in `ALL` (e.g. 32-bit, freebsd) — the caller just
    /// omits the `(host)` marker in that case.
    pub fn from_host_triple(triple: &str) -> Option<Target> {
        let arch = if triple.starts_with("x86_64") || triple.starts_with("amd64") {
            Arch::X86_64
        } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
            Arch::Arm64
        } else {
            return None;
        };
        let os = if triple.contains("linux") {
            Os::Linux
        } else if triple.contains("darwin") || triple.contains("macos") || triple.contains("apple") {
            Os::Macos
        } else if triple.contains("windows") || triple.contains("mingw") || triple.contains("msvc") {
            Os::Windows
        } else {
            return None;
        };
        Some(Target { os, arch })
    }

    /// Resolves user-supplied `-L<dir>` link-search tokens for *this* target.
    ///
    /// For each `-L<dir>`, if a per-target subdirectory `<dir>/<label>` exists
    /// (e.g. `libs/linux-x86_64`), rewrite the token to point at it so that a
    /// `--target all` build picks each target's own single-arch libraries.
    /// When no such subdir exists the original token is kept verbatim, so a
    /// flat `-L<dir>` layout keeps working unchanged (backward compatible).
    /// Non-`-L` tokens (shouldn't occur — `parse_cli` only stores `-L…`) pass
    /// through untouched.
    pub fn resolve_lib_dirs(&self, lib_dirs: &[String]) -> Vec<String> {
        let label = self.label();
        lib_dirs
            .iter()
            .map(|tok| {
                if let Some(dir) = tok.strip_prefix("-L") {
                    let sub = std::path::Path::new(dir).join(&label);
                    if sub.is_dir() {
                        return format!("-L{}", sub.display());
                    }
                }
                tok.clone()
            })
            .collect()
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

    #[test]
    fn from_host_triple_matches_common_hosts() {
        assert_eq!(
            Target::from_host_triple("x86_64-unknown-linux-gnu"),
            Some(Target { os: Os::Linux, arch: Arch::X86_64 })
        );
        assert_eq!(
            Target::from_host_triple("aarch64-apple-darwin"),
            Some(Target { os: Os::Macos, arch: Arch::Arm64 })
        );
        assert_eq!(
            Target::from_host_triple("x86_64-apple-darwin"),
            Some(Target { os: Os::Macos, arch: Arch::X86_64 })
        );
        assert_eq!(
            Target::from_host_triple("x86_64-pc-windows-msvc"),
            Some(Target { os: Os::Windows, arch: Arch::X86_64 })
        );
        assert_eq!(Target::from_host_triple("riscv64-unknown-linux-gnu"), None);
        assert_eq!(Target::from_host_triple("i686-unknown-linux-gnu"), None);
    }

    #[test]
    fn resolve_lib_dirs_prefers_label_subdir_when_present() {
        let tmp = std::env::temp_dir().join(format!("verb_resolve_lib_dirs_{}", std::process::id()));
        let sub = tmp.join("linux-x86_64");
        std::fs::create_dir_all(&sub).unwrap();

        let linux = Target { os: Os::Linux, arch: Arch::X86_64 };
        let dir_arg = format!("-L{}", tmp.display());
        let resolved = linux.resolve_lib_dirs(&[dir_arg.clone()]);
        assert_eq!(resolved, vec![format!("-L{}", sub.display())]);

        // A target with no matching subdir falls back to the bare token.
        let macos = Target { os: Os::Macos, arch: Arch::Arm64 };
        let resolved_fallback = macos.resolve_lib_dirs(&[dir_arg.clone()]);
        assert_eq!(resolved_fallback, vec![dir_arg]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_lib_dirs_keeps_flat_layout_unchanged() {
        let linux = Target { os: Os::Linux, arch: Arch::X86_64 };
        let dirs = vec!["-L/nonexistent/flat/libs".to_string()];
        assert_eq!(linux.resolve_lib_dirs(&dirs), dirs);
    }
}
