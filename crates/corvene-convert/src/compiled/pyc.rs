//! Python bytecode (`.pyc`) as text: the header (Python version, source
//! timestamp or hash) and every code object with its arguments, constants,
//! names and variables, nested functions and classes indented under their
//! parent. The instructions themselves are summed up by size and a hash:
//! their encoding changes with each Python release.

use std::fmt::Write as _;

/// Code objects nested deeper are left out (a malicious file could recurse).
const MAX_DEPTH: usize = 64;
/// Items printed per tuple, list, dict or set constant.
const MAX_ITEMS: usize = 200;

/// The Python release a `.pyc` magic number belongs to.
fn release(magic: u16) -> Option<(u8, u8)> {
    Some(match magic {
        20121 => (1, 5),
        50428 => (1, 6),
        50823 => (2, 0),
        60202 => (2, 1),
        60717 => (2, 2),
        62011 | 62021 => (2, 3),
        62041 | 62051 | 62061 => (2, 4),
        62071..=62131 => (2, 5),
        62151 | 62161 => (2, 6),
        62171..=62211 => (2, 7),
        3000..=3131 => (3, 0),
        3141..=3151 => (3, 1),
        3160..=3180 => (3, 2),
        3190..=3230 => (3, 3),
        3250..=3310 => (3, 4),
        3320..=3351 => (3, 5),
        3360..=3379 => (3, 6),
        3390..=3394 => (3, 7),
        3400..=3413 => (3, 8),
        3420..=3425 => (3, 9),
        3430..=3439 => (3, 10),
        3450..=3495 => (3, 11),
        3500..=3531 => (3, 12),
        3550..=3571 => (3, 13),
        3600..=3699 => (3, 14),
        3700..=3799 => (3, 15),
        _ => return None,
    })
}

#[derive(Clone, Debug)]
enum Value {
    Null,
    None,
    Bool(bool),
    StopIteration,
    Ellipsis,
    Int(String),
    Float(f64),
    Complex(f64, f64),
    Bytes(Vec<u8>),
    Str(String),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Set(Vec<Value>),
    FrozenSet(Vec<Value>),
    Slice(Box<(Value, Value, Value)>),
    Code(Box<Code>),
}

#[derive(Clone, Debug, Default)]
struct Code {
    argcount: i64,
    posonly: i64,
    kwonly: i64,
    flags: i64,
    bytecode: Vec<u8>,
    consts: Vec<Value>,
    names: Vec<String>,
    /// Local variable names (`co_varnames`, or the locals of
    /// `co_localsplusnames` from 3.11).
    varnames: Vec<String>,
    freevars: Vec<String>,
    cellvars: Vec<String>,
    filename: String,
    name: String,
    qualname: String,
    firstlineno: i64,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    version: (u8, u8),
    refs: Vec<Value>,
    depth: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn i32(&mut self) -> Option<i32> {
        let b = self.bytes(4)?;
        Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn len(&mut self) -> Option<usize> {
        let n = self.i32()?;
        usize::try_from(n).ok().filter(|n| *n <= self.data.len())
    }

    fn object(&mut self) -> Option<Value> {
        self.depth += 1;
        if self.depth > MAX_DEPTH * 4 {
            return None;
        }
        let out = self.object_inner();
        self.depth -= 1;
        out
    }

    fn object_inner(&mut self) -> Option<Value> {
        let code = self.u8()?;
        let flag = code & 0x80 != 0;
        let ty = code & 0x7f;
        // a code object refers to itself through its slot: reserve it first
        let slot = if flag {
            self.refs.push(Value::None);
            Some(self.refs.len() - 1)
        } else {
            None
        };
        let value = match ty {
            b'0' => Value::Null,
            b'N' => Value::None,
            b'F' => Value::Bool(false),
            b'T' => Value::Bool(true),
            b'S' => Value::StopIteration,
            b'.' => Value::Ellipsis,
            b'i' => Value::Int(self.i32()?.to_string()),
            b'I' => {
                let b = self.bytes(8)?;
                Value::Int(i64::from_le_bytes(b.try_into().ok()?).to_string())
            }
            b'l' => {
                let n = self.i32()?;
                let digits = n.unsigned_abs() as usize;
                if digits > self.data.len() {
                    return None;
                }
                let mut value: i128 = 0;
                let mut overflow = false;
                for k in 0..digits {
                    let b = self.bytes(2)?;
                    let d = i128::from(u16::from_le_bytes([b[0], b[1]]));
                    if k < 8 {
                        value |= d << (15 * k);
                    } else if d != 0 {
                        overflow = true;
                    }
                }
                if overflow {
                    Value::Int(format!("<{}-digit integer>", digits))
                } else {
                    Value::Int((if n < 0 { -value } else { value }).to_string())
                }
            }
            b'g' => Value::Float(f64::from_le_bytes(self.bytes(8)?.try_into().ok()?)),
            b'y' => {
                let re = f64::from_le_bytes(self.bytes(8)?.try_into().ok()?);
                let im = f64::from_le_bytes(self.bytes(8)?.try_into().ok()?);
                Value::Complex(re, im)
            }
            b'f' => {
                let n = self.u8()? as usize;
                let s = String::from_utf8_lossy(self.bytes(n)?)
                    .parse()
                    .unwrap_or(f64::NAN);
                Value::Float(s)
            }
            b'x' => {
                let n = self.u8()? as usize;
                let re = String::from_utf8_lossy(self.bytes(n)?)
                    .parse()
                    .unwrap_or(f64::NAN);
                let n = self.u8()? as usize;
                let im = String::from_utf8_lossy(self.bytes(n)?)
                    .parse()
                    .unwrap_or(f64::NAN);
                Value::Complex(re, im)
            }
            b's' => {
                let n = self.len()?;
                let b = self.bytes(n)?.to_vec();
                // Python 2 keeps text in byte strings
                if self.version.0 < 3 {
                    Value::Str(String::from_utf8_lossy(&b).into_owned())
                } else {
                    Value::Bytes(b)
                }
            }
            b't' | b'u' | b'a' | b'A' => {
                let n = self.len()?;
                Value::Str(String::from_utf8_lossy(self.bytes(n)?).into_owned())
            }
            b'z' | b'Z' => {
                let n = self.u8()? as usize;
                Value::Str(String::from_utf8_lossy(self.bytes(n)?).into_owned())
            }
            b'R' => {
                // Python 2 interned string reference
                let ix = self.len()?;
                self.refs.get(ix).cloned().unwrap_or(Value::None)
            }
            b'(' => {
                let n = self.len()?;
                Value::Tuple(self.items(n)?)
            }
            b')' => {
                // a small tuple: a one-byte length
                let n = self.u8()? as usize;
                Value::Tuple(self.items(n)?)
            }
            b'[' => {
                let n = self.len()?;
                Value::List(self.items(n)?)
            }
            b'<' => {
                let n = self.len()?;
                Value::Set(self.items(n)?)
            }
            b'>' => {
                let n = self.len()?;
                Value::FrozenSet(self.items(n)?)
            }
            b'{' => {
                let mut pairs = Vec::new();
                loop {
                    let key = self.object()?;
                    if matches!(key, Value::Null) {
                        break;
                    }
                    let value = self.object()?;
                    pairs.push((key, value));
                }
                Value::Dict(pairs)
            }
            b':' => {
                let a = self.object()?;
                let b = self.object()?;
                let c = self.object()?;
                Value::Slice(Box::new((a, b, c)))
            }
            b'r' => {
                let ix = self.len()?;
                return self.refs.get(ix).cloned();
            }
            b'c' => Value::Code(Box::new(self.code()?)),
            _ => return None,
        };
        if let Some(slot) = slot {
            self.refs[slot] = value.clone();
        }
        Some(value)
    }

    fn items(&mut self, n: usize) -> Option<Vec<Value>> {
        let mut out = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            out.push(self.object()?);
        }
        Some(out)
    }

    fn strings(&mut self) -> Option<Vec<String>> {
        Some(match self.object()? {
            Value::Tuple(items) | Value::List(items) => items
                .into_iter()
                .map(|v| match v {
                    Value::Str(s) => s,
                    other => short(&other),
                })
                .collect(),
            _ => Vec::new(),
        })
    }

    fn string(&mut self) -> Option<String> {
        Some(match self.object()? {
            Value::Str(s) => s,
            Value::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
            _ => String::new(),
        })
    }

    fn bytes_obj(&mut self) -> Option<Vec<u8>> {
        Some(match self.object()? {
            Value::Bytes(b) => b,
            Value::Str(s) => s.into_bytes(),
            _ => Vec::new(),
        })
    }

    fn code(&mut self) -> Option<Code> {
        let (major, minor) = self.version;
        let mut code = Code::default();
        if major < 3 {
            code.argcount = self.i32()?.into();
            self.i32()?; // nlocals
            self.i32()?; // stacksize
            code.flags = self.i32()?.into();
        } else {
            code.argcount = self.i32()?.into();
            if minor >= 8 {
                code.posonly = self.i32()?.into();
            }
            code.kwonly = self.i32()?.into();
            if minor < 11 {
                self.i32()?; // nlocals
            }
            self.i32()?; // stacksize
            code.flags = self.i32()?.into();
        }
        code.bytecode = self.bytes_obj()?;
        code.consts = match self.object()? {
            Value::Tuple(items) | Value::List(items) => items,
            _ => Vec::new(),
        };
        code.names = self.strings()?;
        if major >= 3 && minor >= 11 {
            let names = self.strings()?;
            let kinds = self.bytes_obj()?;
            for (name, kind) in names.into_iter().zip(kinds) {
                // CO_FAST_LOCAL 0x20, CO_FAST_CELL 0x40, CO_FAST_FREE 0x80
                if kind & 0x80 != 0 {
                    code.freevars.push(name);
                } else if kind & 0x40 != 0 {
                    code.cellvars.push(name);
                } else {
                    code.varnames.push(name);
                }
            }
        } else {
            code.varnames = self.strings()?;
            code.freevars = self.strings()?;
            code.cellvars = self.strings()?;
        }
        code.filename = self.string()?;
        code.name = self.string()?;
        if major >= 3 && minor >= 11 {
            code.qualname = self.string()?;
        }
        code.firstlineno = self.i32()?.into();
        self.object()?; // line table
        if major >= 3 && minor >= 11 {
            self.object()?; // exception table
        }
        Some(code)
    }
}

/// A constant as Python's `repr` would show it (shortened).
fn short(value: &Value) -> String {
    fn seq(items: &[Value], open: &str, close: &str) -> String {
        let mut parts: Vec<String> = items.iter().take(MAX_ITEMS).map(short).collect();
        if items.len() > MAX_ITEMS {
            parts.push(format!("… {} more", items.len() - MAX_ITEMS));
        }
        let trailing = if open == "(" && items.len() == 1 {
            ","
        } else {
            ""
        };
        format!("{open}{}{trailing}{close}", parts.join(", "))
    }
    match value {
        Value::Null => "NULL".into(),
        Value::None => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::StopIteration => "StopIteration".into(),
        Value::Ellipsis => "Ellipsis".into(),
        Value::Int(s) => s.clone(),
        Value::Float(f) => format!("{f:?}"),
        Value::Complex(re, im) => format!("({re:?}+{im:?}j)"),
        Value::Bytes(b) => format!("b{}", quote_py(&String::from_utf8_lossy(b))),
        Value::Str(s) => quote_py(s),
        Value::Tuple(items) => seq(items, "(", ")"),
        Value::List(items) => seq(items, "[", "]"),
        Value::Set(items) => seq(items, "{", "}"),
        Value::FrozenSet(items) => format!("frozenset({})", seq(items, "{", "}")),
        Value::Dict(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .take(MAX_ITEMS)
                .map(|(k, v)| format!("{}: {}", short(k), short(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Value::Slice(s) => format!("slice({}, {}, {})", short(&s.0), short(&s.1), short(&s.2)),
        Value::Code(code) => format!("<code {}>", code.name),
    }
}

fn quote_py(s: &str) -> String {
    let mut out = String::from("'");
    for ch in s.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// The `co_flags` that say what kind of function it is.
fn flag_names(flags: i64) -> Vec<&'static str> {
    let table = [
        (0x0004, "varargs"),
        (0x0008, "varkeywords"),
        (0x0020, "generator"),
        (0x0080, "coroutine"),
        (0x0100, "iterable coroutine"),
        (0x0200, "async generator"),
    ];
    table
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, n)| *n)
        .collect()
}

/// FNV-1a, to tell instruction sequences apart in a short form.
fn fnv(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in bytes {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn write_code(out: &mut String, code: &Code, depth: usize) {
    let pad = "  ".repeat(depth);
    let title = if code.qualname.is_empty() {
        &code.name
    } else {
        &code.qualname
    };
    let _ = writeln!(out, "{pad}{title} (line {})", code.firstlineno);
    let flags = flag_names(code.flags);
    if !flags.is_empty() {
        let _ = writeln!(out, "{pad}  flags: {}", flags.join(", "));
    }
    let positional = (code.argcount + code.kwonly).max(0) as usize;
    if positional > 0 || code.flags & 0x0c != 0 {
        let args = arguments(code);
        let _ = writeln!(out, "{pad}  arguments: {}", args.join(", "));
    }
    let consts: Vec<String> = code
        .consts
        .iter()
        .filter(|c| !matches!(c, Value::Code(_)))
        .map(short)
        .collect();
    if !consts.is_empty() {
        let _ = writeln!(out, "{pad}  constants:");
        for c in consts {
            let _ = writeln!(out, "{pad}    {c}");
        }
    }
    for (label, list) in [
        ("names", &code.names),
        ("variables", &code.varnames),
        ("free variables", &code.freevars),
        ("cell variables", &code.cellvars),
    ] {
        if !list.is_empty() {
            let _ = writeln!(out, "{pad}  {label}: {}", list.join(", "));
        }
    }
    let _ = writeln!(
        out,
        "{pad}  bytecode: {} bytes ({:08x})",
        code.bytecode.len(),
        fnv(&code.bytecode)
    );
    if depth < MAX_DEPTH {
        for c in &code.consts {
            if let Value::Code(inner) = c {
                out.push('\n');
                write_code(out, inner, depth + 1);
            }
        }
    }
}

/// What a `.pyc` header says about its source.
enum Stamp {
    /// The source's hash, and whether Python checks it.
    Hash(Vec<u8>, bool),
    /// The source's modification time and size.
    Time(Option<u32>, Option<u32>),
}

struct Parsed {
    magic: u16,
    version: (u8, u8),
    stamp: Stamp,
    code: Code,
}

fn parse(bytes: &[u8]) -> Result<Parsed, String> {
    if bytes.len() < 8 || &bytes[2..4] != b"\r\n" {
        return Err("Not a Python bytecode file.".into());
    }
    let magic = u16::from_le_bytes([bytes[0], bytes[1]]);
    let version =
        release(magic).ok_or_else(|| format!("Unknown Python bytecode version {magic}."))?;
    let word = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (stamp, start) = if version >= (3, 7) {
        let flags = word(4).ok_or("Truncated header.")?;
        if flags & 1 != 0 {
            let hash = bytes.get(8..16).ok_or("Truncated header.")?;
            (Stamp::Hash(hash.to_vec(), flags & 2 != 0), 16)
        } else {
            (Stamp::Time(word(8), word(12)), 16)
        }
    } else if version >= (3, 3) {
        (Stamp::Time(word(4), word(8)), 12)
    } else {
        (Stamp::Time(word(4), None), 8)
    };
    let mut reader = Reader {
        data: bytes,
        pos: start,
        version,
        refs: Vec::new(),
        depth: 0,
    };
    let Some(Value::Code(code)) = reader.object() else {
        return Err("The bytecode could not be read.".into());
    };
    Ok(Parsed {
        magic,
        version,
        stamp,
        code: *code,
    })
}

/// The text of one `.pyc` file.
pub fn to_text(bytes: &[u8]) -> Result<String, String> {
    let Parsed {
        magic,
        version,
        stamp,
        code,
    } = parse(bytes)?;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Python {}.{} bytecode (magic {magic})",
        version.0, version.1
    );
    match stamp {
        Stamp::Hash(hash, checked) => {
            let _ = writeln!(
                out,
                "Source hash: {}{}",
                crate::out::hex(&hash),
                if checked { " (checked)" } else { "" }
            );
        }
        Stamp::Time(mtime, size) => source_stamp(&mut out, mtime, size),
    }
    if !code.filename.is_empty() {
        let _ = writeln!(out, "Compiled from {}", code.filename);
    }
    out.push('\n');
    write_code(&mut out, &code, 0);
    Ok(out)
}

/// `1360-compiled-summary`: one `.pyc` file as facts and a table of its
/// code objects.
pub fn summary(bytes: &[u8]) -> Result<super::summary::Side, String> {
    use super::summary::Side;
    let Parsed {
        magic,
        version,
        stamp,
        code,
    } = parse(bytes)?;
    let mut side = Side {
        title: format!("Python {}.{} bytecode", version.0, version.1),
        ..Side::default()
    };
    side.fact("Python", format!("{}.{}", version.0, version.1));
    side.fact("Magic number", magic.to_string());
    match stamp {
        Stamp::Hash(hash, checked) => {
            side.fact(
                "Source hash",
                format!(
                    "{}{}",
                    crate::out::hex(&hash),
                    if checked { " (checked)" } else { "" }
                ),
            );
        }
        Stamp::Time(mtime, size) => {
            if let Some(mtime) = mtime {
                side.fact("Source modified", utc(i64::from(mtime)));
            }
            if let Some(size) = size {
                side.bytes("Source size", u64::from(size));
            }
        }
    }
    side.fact("Compiled from", code.filename.clone());
    side.bytes("Size", bytes.len() as u64);
    code_rows(&mut side, &code, "", 0);
    let n = side.groups.first().map_or(0, |(_, e)| e.len());
    side.count("Code objects", n as u64);
    Ok(side)
}

/// The arguments of a code object as a signature (`a, /, b, *args`).
fn arguments(code: &Code) -> Vec<String> {
    let positional = (code.argcount + code.kwonly).max(0) as usize;
    let mut args: Vec<String> = Vec::new();
    for (ix, name) in code.varnames.iter().take(positional).enumerate() {
        if ix as i64 == code.argcount && code.kwonly > 0 {
            args.push("*".into());
        }
        args.push(name.clone());
        if code.posonly > 0 && ix as i64 + 1 == code.posonly {
            args.push("/".into());
        }
    }
    let mut next = positional;
    if code.flags & 0x04 != 0 {
        if let Some(name) = code.varnames.get(next) {
            args.push(format!("*{name}"));
        }
        next += 1;
    }
    if code.flags & 0x08 != 0
        && let Some(name) = code.varnames.get(next)
    {
        args.push(format!("**{name}"));
    }
    args
}

fn code_rows(side: &mut super::summary::Side, code: &Code, parent: &str, depth: usize) {
    use super::summary::{Entry, hash};
    use corvene_models::compiled::CompiledGroupKind as G;
    // before 3.11 there is no qualified name: make one from the nesting
    let name = if !code.qualname.is_empty() {
        code.qualname.clone()
    } else if parent.is_empty() || parent == "<module>" {
        code.name.clone()
    } else {
        format!("{parent}.{}", code.name)
    };
    let consts: Vec<String> = code
        .consts
        .iter()
        .filter(|c| !matches!(c, Value::Code(_)))
        .map(short)
        .collect();
    let mut detail = format!("({})", arguments(code).join(", "));
    let flags = flag_names(code.flags);
    if !flags.is_empty() {
        detail = format!("{} {detail}", flags.join(" "));
    }
    detail.push_str(&format!(
        " · {} {} · {} {}",
        consts.len(),
        if consts.len() == 1 {
            "constant"
        } else {
            "constants"
        },
        code.names.len(),
        if code.names.len() == 1 {
            "name"
        } else {
            "names"
        },
    ));
    let mut body = code.bytecode.clone();
    body.extend(consts.join("\n").as_bytes());
    body.extend(code.names.join("\n").as_bytes());
    side.push(
        G::CodeObjects,
        Entry::new(name.clone())
            .detail(detail)
            .size(code.bytecode.len() as u64)
            .body(hash(&body)),
    );
    if depth < MAX_DEPTH {
        for c in &code.consts {
            if let Value::Code(inner) = c {
                code_rows(side, inner, &name, depth + 1);
            }
        }
    }
}

fn source_stamp(out: &mut String, mtime: Option<u32>, size: Option<u32>) {
    if let Some(mtime) = mtime {
        let _ = write!(out, "Source modified: {}", utc(i64::from(mtime)));
    }
    if let Some(size) = size {
        let _ = write!(out, ", {} bytes", crate::out::thousands(u64::from(size)));
    }
    out.push('\n');
}

/// A Unix time as `YYYY-MM-DD hh:mm:ss UTC`.
pub(crate) fn utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `x = 1` compiled by CPython 3.12 (`py_compile`, unchecked hash off).
    fn sample_312() -> Vec<u8> {
        let mut b = vec![0xcb, 0x0d, 0x0d, 0x0a, 0, 0, 0, 0];
        b.extend(1_700_000_000u32.to_le_bytes());
        b.extend(6u32.to_le_bytes());
        // code object
        b.push(b'c' | 0x80);
        for v in [0i32, 0, 0, 1, 0] {
            b.extend(v.to_le_bytes()); // argcount posonly kwonly stacksize flags
        }
        // bytecode
        b.push(b's');
        b.extend(4i32.to_le_bytes());
        b.extend([0x97, 0x00, 0x64, 0x00]);
        // consts (1, None)
        b.push(b')');
        b.push(2);
        b.push(b'i');
        b.extend(1i32.to_le_bytes());
        b.push(b'N');
        // names ('x',)
        b.push(b')');
        b.push(1);
        b.push(b'Z');
        b.push(1);
        b.push(b'x');
        // localsplusnames () kinds b''
        b.push(b')');
        b.push(0);
        b.push(b's');
        b.extend(0i32.to_le_bytes());
        // filename, name, qualname
        for s in ["a.py", "<module>", "<module>"] {
            b.push(b'z');
            b.push(s.len() as u8);
            b.extend(s.as_bytes());
        }
        b.extend(1i32.to_le_bytes());
        b.push(b's');
        b.extend(0i32.to_le_bytes());
        b.push(b's');
        b.extend(0i32.to_le_bytes());
        b
    }

    #[test]
    fn reads_a_312_module() {
        let text = to_text(&sample_312()).unwrap();
        assert!(
            text.starts_with("Python 3.12 bytecode (magic 3531)"),
            "{text}"
        );
        assert!(
            text.contains("Source modified: 2023-11-14 22:13:20 UTC, 6 bytes"),
            "{text}"
        );
        assert!(text.contains("<module> (line 1)"), "{text}");
        assert!(text.contains("    1\n    None"), "{text}");
        assert!(text.contains("names: x"), "{text}");
    }

    #[test]
    fn summarizes_a_312_module() {
        let side = summary(&sample_312()).unwrap();
        assert_eq!(side.title, "Python 3.12 bytecode");
        let (_, rows) = &side.groups[0];
        assert_eq!(rows[0].name, "<module>");
        assert_eq!(rows[0].detail, "() · 2 constants · 1 name");
        assert_eq!(rows[0].size, Some(4));
    }

    #[test]
    fn refuses_other_files() {
        assert!(to_text(b"\x7fELF....").is_err());
        assert!(
            to_text(&[
                0xcb, 0x0d, 0x0d, 0x0a, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'Q'
            ])
            .is_err()
        );
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00 UTC");
    }
}
