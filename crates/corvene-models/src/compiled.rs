//! Corvene `1360-compiled-summary`: a compiled file (native binary, Java
//! class file, Python bytecode, WebAssembly module) compared as structured
//! data: a header of facts (format, architecture, entry point, version)
//! with their old and new values, then tables of its parts (sections,
//! linked libraries, symbols, members, methods, code objects, imports,
//! exports) marked added, removed or changed.
//!
//! GitHub Desktop shows compiled files as binary files
//! (`app/src/ui/diff/index.tsx`).

use serde::{Deserialize, Serialize};

/// What kind of compiled file was read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompiledFamily {
    /// Mach-O, ELF, PE/COFF, static libraries.
    #[default]
    Native,
    JavaClass,
    PythonBytecode,
    WebAssembly,
}

/// One table of a compiled file's parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompiledGroupKind {
    /// Architectures of a universal binary.
    Architectures,
    Segments,
    Sections,
    /// Linked (needed) libraries.
    Libraries,
    /// Run paths (`LC_RPATH`, `DT_RUNPATH`).
    RunPaths,
    /// Symbols the file defines (exports).
    DefinedSymbols,
    /// Symbols the file needs from elsewhere (imports).
    ImportedSymbols,
    /// Members of a static library.
    Members,
    /// A class's interfaces.
    Interfaces,
    Fields,
    Methods,
    /// Python code objects (the module, its functions and classes).
    CodeObjects,
    /// WebAssembly imports.
    Imports,
    /// WebAssembly exports.
    Exports,
    /// WebAssembly functions.
    Functions,
    Memories,
    Tables,
    Globals,
    /// WebAssembly custom sections.
    CustomSections,
}

impl CompiledGroupKind {
    /// The table's title.
    pub fn title(self) -> &'static str {
        use CompiledGroupKind::*;
        match self {
            Architectures => "Architectures",
            Segments => "Segments",
            Sections => "Sections",
            Libraries => "Linked libraries",
            RunPaths => "Run paths",
            DefinedSymbols => "Defined symbols",
            ImportedSymbols => "Imported symbols",
            Members => "Members",
            Interfaces => "Interfaces",
            Fields => "Fields",
            Methods => "Methods",
            CodeObjects => "Code objects",
            Imports => "Imports",
            Exports => "Exports",
            Functions => "Functions",
            Memories => "Memories",
            Tables => "Tables",
            Globals => "Globals",
            CustomSections => "Custom sections",
        }
    }

    /// What a changed row's contents are called ("bytecode changed").
    pub fn body_word(self) -> &'static str {
        use CompiledGroupKind::*;
        match self {
            Methods | CodeObjects => "bytecode changed",
            Functions => "code changed",
            Fields | Globals => "value changed",
            _ => "contents changed",
        }
    }

    /// Whether the table has a size column.
    pub fn has_size(self) -> bool {
        use CompiledGroupKind::*;
        matches!(
            self,
            Architectures
                | Segments
                | Sections
                | DefinedSymbols
                | Members
                | Methods
                | CodeObjects
                | Functions
                | CustomSections
        )
    }
}

/// How a row (or fact) differs between the two versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CompiledStatus {
    Added,
    Removed,
    Changed,
    Unchanged,
}

impl CompiledStatus {
    pub fn label(self) -> &'static str {
        match self {
            CompiledStatus::Added => "Added",
            CompiledStatus::Removed => "Removed",
            CompiledStatus::Changed => "Changed",
            CompiledStatus::Unchanged => "Unchanged",
        }
    }
}

/// A header value on both sides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledFact {
    pub label: String,
    pub old: Option<String>,
    pub new: Option<String>,
    /// new - old, for a numeric fact present on both sides.
    pub delta: Option<i64>,
    /// The numbers are byte counts.
    pub bytes: bool,
}

impl CompiledFact {
    pub fn status(&self) -> CompiledStatus {
        match (&self.old, &self.new) {
            (None, Some(_)) => CompiledStatus::Added,
            (Some(_), None) => CompiledStatus::Removed,
            (Some(a), Some(b)) if a != b => CompiledStatus::Changed,
            _ => CompiledStatus::Unchanged,
        }
    }
}

/// One part (section, symbol, method) on both sides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledRow {
    pub status: CompiledStatus,
    pub name: String,
    /// The part's kind, type or modifiers (the new side's, or the old one's
    /// when removed).
    pub detail: String,
    /// The old side's detail, when it differs.
    pub old_detail: Option<String>,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
    /// The contents changed under the same name and size.
    pub body_changed: bool,
}

impl CompiledRow {
    /// new - old size, when both are known and differ.
    pub fn size_delta(&self) -> Option<i64> {
        match (self.old_size, self.new_size) {
            (Some(a), Some(b)) if a != b => Some(b as i64 - a as i64),
            _ => None,
        }
    }
}

/// One table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledGroup {
    pub kind: CompiledGroupKind,
    /// Changed rows first in the file's order, unchanged ones (up to a
    /// cap) after.
    pub rows: Vec<CompiledRow>,
    /// Unchanged rows left out of `rows` (past the cap).
    pub unchanged_hidden: usize,
    pub old_count: usize,
    pub new_count: usize,
}

impl CompiledGroup {
    pub fn count(&self, status: CompiledStatus) -> usize {
        let listed = self.rows.iter().filter(|r| r.status == status).count();
        if status == CompiledStatus::Unchanged {
            listed + self.unchanged_hidden
        } else {
            listed
        }
    }

    pub fn changed(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.status != CompiledStatus::Unchanged)
            .count()
    }
}

/// Both versions of a compiled file compared.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledDiff {
    pub family: CompiledFamily,
    /// A side's headline ("Mach-O 64-bit dynamic library, arm64").
    pub old_title: Option<String>,
    pub new_title: Option<String>,
    pub facts: Vec<CompiledFact>,
    pub groups: Vec<CompiledGroup>,
}

impl CompiledDiff {
    /// Rows added, removed and changed across all tables.
    pub fn counts(&self) -> [usize; 3] {
        let mut out = [0; 3];
        for row in self.groups.iter().flat_map(|g| &g.rows) {
            match row.status {
                CompiledStatus::Added => out[0] += 1,
                CompiledStatus::Removed => out[1] += 1,
                CompiledStatus::Changed => out[2] += 1,
                CompiledStatus::Unchanged => {}
            }
        }
        out
    }

    /// An estimate of the bytes held (for the diff cache).
    pub fn weight(&self) -> usize {
        self.facts
            .iter()
            .map(|f| 48 + f.label.len() + f.old.as_ref().map_or(0, String::len) * 2)
            .sum::<usize>()
            + self
                .groups
                .iter()
                .flat_map(|g| &g.rows)
                .map(|r| 64 + r.name.len() + r.detail.len())
                .sum::<usize>()
    }
}
