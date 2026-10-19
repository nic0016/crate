//! CAD-specific knowledge: file categories and files that must never be versioned.

use std::fmt;

/// Patterns (gitignore syntax, case-insensitive) that are always ignored.
pub const DEFAULT_IGNORES: &[&str] = &[
    // SolidWorks / Office lock files while a document is open
    "~$*",
    // Temporary and backup files
    "*.tmp",
    "~*.tmp",
    "*.bak",
    "*.swbak",
    "*.lck",
    "*.swp",
    "*.crate-tmp",
    // Inventor keeps automatic old versions in these folders
    "OldVersions/",
    // Windows / macOS clutter
    "*.lnk",
    "Thumbs.db",
    "desktop.ini",
    ".DS_Store",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Assembly,
    Part,
    Drawing,
    Neutral,
    Export,
    Archive,
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Assembly,
        Category::Part,
        Category::Drawing,
        Category::Neutral,
        Category::Export,
        Category::Archive,
        Category::Other,
    ];

    pub fn of(path: &str) -> Self {
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "sldasm" | "iam" | "asm" | "catproduct" | "f3z" => Category::Assembly,
            "sldprt" | "ipt" | "prt" | "catpart" | "par" | "f3d" | "fcstd" => Category::Part,
            "slddrw" | "idw" | "dwg" | "drw" | "catdrawing" => Category::Drawing,
            "step" | "stp" | "iges" | "igs" | "x_t" | "x_b" | "sat" | "jt" | "3mf" => Category::Neutral,
            "pdf" | "dxf" | "stl" | "glb" | "gltf" | "obj" | "png" | "jpg" | "jpeg" | "edrw" | "eprt"
            | "easm" => Category::Export,
            "zip" | "7z" | "rar" | "pack" => Category::Archive,
            _ => Category::Other,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Category::Assembly => "Baugruppe",
            Category::Part => "Teil",
            Category::Drawing => "Zeichnung",
            Category::Neutral => "Neutralformat",
            Category::Export => "Export",
            Category::Archive => "Archiv",
            Category::Other => "Sonstiges",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

pub fn human_delta(old: u64, new: u64) -> String {
    if new >= old {
        format!("+{}", human_size(new - old))
    } else {
        format!("-{}", human_size(old - new))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(Category::of("Rahmen/Hauptbaugruppe.SLDASM"), Category::Assembly);
        assert_eq!(Category::of("teil.SLDPRT"), Category::Part);
        assert_eq!(Category::of("x.slddrw"), Category::Drawing);
        assert_eq!(Category::of("export.STEP"), Category::Neutral);
        assert_eq!(Category::of("blech.DXF"), Category::Export);
        assert_eq!(Category::of("README"), Category::Other);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_delta(2048, 1024), "-1.0 KB");
    }
}
