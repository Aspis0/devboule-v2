//! The ordered catalog of editor targets — the caret menu's preference
//! order, each entry with the family that spells its launch.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    Vscode,
    Zed,
    JetBrains,
}

pub(crate) struct TargetSpec {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) family: Family,
    pub(crate) cli: &'static [&'static str],
    /// The product executables a `bin\` script's directory stands beside.
    pub(crate) win_exe: &'static [&'static str],
    /// Known install locations: (environment base, path under it).
    pub(crate) win: &'static [(WinBase, &'static str)],
    /// macOS application bundle names, without `.app`.
    pub(crate) mac: &'static [&'static str],
}

pub(crate) enum WinBase {
    LocalAppData,
    ProgramFiles,
}

// Order and launch-argument forms follow Paseo's editor targets (packages/desktop/src/features/editor-targets).
pub(crate) const SPECS: &[TargetSpec] = &[
    TargetSpec {
        id: "cursor",
        label: "Cursor",
        family: Family::Vscode,
        cli: &["cursor"],
        win_exe: &["Cursor.exe"],
        win: &[(WinBase::LocalAppData, "Programs\\cursor\\Cursor.exe")],
        mac: &["Cursor"],
    },
    TargetSpec {
        id: "vscode",
        label: "VS Code",
        family: Family::Vscode,
        cli: &["code"],
        win_exe: &["Code.exe"],
        win: &[
            (
                WinBase::LocalAppData,
                "Programs\\Microsoft VS Code\\Code.exe",
            ),
            (WinBase::ProgramFiles, "Microsoft VS Code\\Code.exe"),
        ],
        mac: &["Visual Studio Code"],
    },
    TargetSpec {
        id: "vscode-insiders",
        label: "VS Code Insiders",
        family: Family::Vscode,
        cli: &["code-insiders"],
        win_exe: &["Code - Insiders.exe"],
        win: &[(
            WinBase::LocalAppData,
            "Programs\\Microsoft VS Code Insiders\\Code - Insiders.exe",
        )],
        mac: &["Visual Studio Code - Insiders"],
    },
    TargetSpec {
        id: "vscodium",
        label: "VSCodium",
        family: Family::Vscode,
        cli: &["codium"],
        win_exe: &["VSCodium.exe"],
        win: &[
            (WinBase::LocalAppData, "Programs\\VSCodium\\VSCodium.exe"),
            (WinBase::ProgramFiles, "VSCodium\\VSCodium.exe"),
        ],
        mac: &["VSCodium"],
    },
    TargetSpec {
        id: "zed",
        label: "Zed",
        family: Family::Zed,
        cli: &["zed", "zeditor"],
        win_exe: &["zed.exe"],
        win: &[(WinBase::LocalAppData, "Zed\\zed.exe")],
        mac: &["Zed"],
    },
    // The JetBrains IDEs: PATH launchers only, never a registry or an
    // install-directory sweep — the smallest portable baseline.
    TargetSpec {
        id: "idea",
        label: "IntelliJ IDEA",
        family: Family::JetBrains,
        cli: &["idea", "idea64"],
        win_exe: &["idea64.exe", "idea.exe"],
        win: &[],
        mac: &["IntelliJ IDEA"],
    },
    TargetSpec {
        id: "webstorm",
        label: "WebStorm",
        family: Family::JetBrains,
        cli: &["webstorm", "webstorm64"],
        win_exe: &["webstorm64.exe", "webstorm.exe"],
        win: &[],
        mac: &["WebStorm"],
    },
    TargetSpec {
        id: "pycharm",
        label: "PyCharm",
        family: Family::JetBrains,
        cli: &["pycharm", "pycharm64"],
        win_exe: &["pycharm64.exe", "pycharm.exe"],
        win: &[],
        mac: &["PyCharm"],
    },
    TargetSpec {
        id: "rustrover",
        label: "RustRover",
        family: Family::JetBrains,
        cli: &["rustrover", "rustrover64"],
        win_exe: &["rustrover64.exe", "rustrover.exe"],
        win: &[],
        mac: &["RustRover"],
    },
    TargetSpec {
        id: "goland",
        label: "GoLand",
        family: Family::JetBrains,
        cli: &["goland", "goland64"],
        win_exe: &["goland64.exe", "goland.exe"],
        win: &[],
        mac: &["GoLand"],
    },
    TargetSpec {
        id: "clion",
        label: "CLion",
        family: Family::JetBrains,
        cli: &["clion", "clion64"],
        win_exe: &["clion64.exe", "clion.exe"],
        win: &[],
        mac: &["CLion"],
    },
    TargetSpec {
        id: "rider",
        label: "Rider",
        family: Family::JetBrains,
        cli: &["rider", "rider64"],
        win_exe: &["rider64.exe", "rider.exe"],
        win: &[],
        mac: &["Rider"],
    },
    TargetSpec {
        id: "phpstorm",
        label: "PhpStorm",
        family: Family::JetBrains,
        cli: &["phpstorm", "phpstorm64"],
        win_exe: &["phpstorm64.exe", "phpstorm.exe"],
        win: &[],
        mac: &["PhpStorm"],
    },
    TargetSpec {
        id: "rubymine",
        label: "RubyMine",
        family: Family::JetBrains,
        cli: &["rubymine", "rubymine64"],
        win_exe: &["rubymine64.exe", "rubymine.exe"],
        win: &[],
        mac: &["RubyMine"],
    },
    TargetSpec {
        id: "datagrip",
        label: "DataGrip",
        family: Family::JetBrains,
        cli: &["datagrip", "datagrip64"],
        win_exe: &["datagrip64.exe", "datagrip.exe"],
        win: &[],
        mac: &["DataGrip"],
    },
];
