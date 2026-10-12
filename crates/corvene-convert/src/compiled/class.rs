//! Java class files as a `javap -c -p`-like listing: the class with its
//! modifiers, supertypes and generic signature, every field and method,
//! and each method's bytecode with its constants resolved.
//!
//! Made to diff well: jump targets are labels (`L0:`) rather than bytecode
//! offsets, and line number and stack map tables are left out, since one
//! added statement shifts all of them.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Whether `bytes` start like a class file (`CAFEBABE` then a class file
/// version, major 45 or later).
pub fn is_class(bytes: &[u8]) -> bool {
    bytes.len() >= 10
        && bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
        && u16::from_be_bytes([bytes[6], bytes[7]]) >= 45
}

/// The listing of one class file.
pub fn to_text(bytes: &[u8]) -> Result<String, String> {
    if !is_class(bytes) {
        return Err("Not a Java class file.".into());
    }
    let class = parse(bytes).ok_or("Not a readable Java class file.")?;
    Ok(class.listing())
}

#[derive(Clone, Debug)]
enum Constant {
    Unused,
    Utf8(String),
    Integer(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class(u16),
    String(u16),
    Field(u16, u16),
    Method(u16, u16),
    InterfaceMethod(u16, u16),
    NameAndType(u16, u16),
    MethodHandle(u8, u16),
    MethodType(u16),
    Dynamic(u16, u16),
    InvokeDynamic(u16, u16),
    Module(u16),
    Package(u16),
}

struct Attribute<'a> {
    name: String,
    data: &'a [u8],
}

struct Member<'a> {
    access: u16,
    name: String,
    descriptor: String,
    attributes: Vec<Attribute<'a>>,
}

struct Class<'a> {
    minor: u16,
    major: u16,
    pool: Vec<Constant>,
    access: u16,
    this: u16,
    super_class: u16,
    interfaces: Vec<u16>,
    fields: Vec<Member<'a>>,
    methods: Vec<Member<'a>>,
    attributes: Vec<Attribute<'a>>,
}

/// A big-endian reader over a byte slice.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }
    fn u1(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn u2(&mut self) -> Option<u16> {
        let b = self.bytes(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u4(&mut self) -> Option<u32> {
        let b = self.bytes(4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i1(&mut self) -> Option<i8> {
        self.u1().map(|v| v as i8)
    }
    fn i2(&mut self) -> Option<i16> {
        self.u2().map(|v| v as i16)
    }
    fn i4(&mut self) -> Option<i32> {
        self.u4().map(|v| v as i32)
    }
}

/// Java's modified UTF-8 (CESU-8 surrogates, `C0 80` for NUL).
fn modified_utf8(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b & 0x80 == 0 {
            units.push(u16::from(b));
            i += 1;
        } else if b & 0xe0 == 0xc0 && i + 1 < bytes.len() {
            units.push((u16::from(b & 0x1f) << 6) | u16::from(bytes[i + 1] & 0x3f));
            i += 2;
        } else if b & 0xf0 == 0xe0 && i + 2 < bytes.len() {
            units.push(
                (u16::from(b & 0x0f) << 12)
                    | (u16::from(bytes[i + 1] & 0x3f) << 6)
                    | u16::from(bytes[i + 2] & 0x3f),
            );
            i += 3;
        } else {
            units.push(0xfffd);
            i += 1;
        }
    }
    String::from_utf16_lossy(&units)
}

fn parse(bytes: &[u8]) -> Option<Class<'_>> {
    let mut r = Reader::new(bytes);
    r.u4()?;
    let minor = r.u2()?;
    let major = r.u2()?;
    let count = r.u2()? as usize;
    let mut pool = vec![Constant::Unused; count.max(1)];
    let mut ix = 1;
    while ix < count {
        let tag = r.u1()?;
        let constant = match tag {
            1 => {
                let len = r.u2()? as usize;
                Constant::Utf8(modified_utf8(r.bytes(len)?))
            }
            3 => Constant::Integer(r.i4()?),
            4 => Constant::Float(f32::from_bits(r.u4()?)),
            5 => Constant::Long(((u64::from(r.u4()?) << 32) | u64::from(r.u4()?)) as i64),
            6 => Constant::Double(f64::from_bits(
                (u64::from(r.u4()?) << 32) | u64::from(r.u4()?),
            )),
            7 => Constant::Class(r.u2()?),
            8 => Constant::String(r.u2()?),
            9 => Constant::Field(r.u2()?, r.u2()?),
            10 => Constant::Method(r.u2()?, r.u2()?),
            11 => Constant::InterfaceMethod(r.u2()?, r.u2()?),
            12 => Constant::NameAndType(r.u2()?, r.u2()?),
            15 => Constant::MethodHandle(r.u1()?, r.u2()?),
            16 => Constant::MethodType(r.u2()?),
            17 => Constant::Dynamic(r.u2()?, r.u2()?),
            18 => Constant::InvokeDynamic(r.u2()?, r.u2()?),
            19 => Constant::Module(r.u2()?),
            20 => Constant::Package(r.u2()?),
            _ => return None,
        };
        let wide = matches!(constant, Constant::Long(_) | Constant::Double(_));
        pool[ix] = constant;
        ix += if wide { 2 } else { 1 };
    }
    let access = r.u2()?;
    let this = r.u2()?;
    let super_class = r.u2()?;
    let n = r.u2()? as usize;
    let mut interfaces = Vec::with_capacity(n);
    for _ in 0..n {
        interfaces.push(r.u2()?);
    }
    let fields = read_members(&mut r, &pool)?;
    let methods = read_members(&mut r, &pool)?;
    let class_attributes = read_attributes(&mut r, &pool)?;
    Some(Class {
        minor,
        major,
        pool,
        access,
        this,
        super_class,
        interfaces,
        fields,
        methods,
        attributes: class_attributes,
    })
}

fn pool_utf8(pool: &[Constant], ix: u16) -> String {
    match pool.get(ix as usize) {
        Some(Constant::Utf8(s)) => s.clone(),
        _ => String::new(),
    }
}

fn read_attributes<'a>(r: &mut Reader<'a>, pool: &[Constant]) -> Option<Vec<Attribute<'a>>> {
    let n = r.u2()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let name = pool_utf8(pool, r.u2()?);
        let len = r.u4()? as usize;
        out.push(Attribute {
            name,
            data: r.bytes(len)?,
        });
    }
    Some(out)
}

fn read_members<'a>(r: &mut Reader<'a>, pool: &[Constant]) -> Option<Vec<Member<'a>>> {
    let n = r.u2()? as usize;
    let mut out = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let access = r.u2()?;
        let name = pool_utf8(pool, r.u2()?);
        let descriptor = pool_utf8(pool, r.u2()?);
        out.push(Member {
            access,
            name,
            descriptor,
            attributes: read_attributes(r, pool)?,
        });
    }
    Some(out)
}

/// A field descriptor (`[Ljava/lang/String;`) as a Java type
/// (`java.lang.String[]`), from `pos`; returns the end.
fn field_type(desc: &[u8], mut pos: usize) -> (String, usize) {
    let mut dims = 0;
    while desc.get(pos) == Some(&b'[') {
        dims += 1;
        pos += 1;
    }
    let (base, end) = match desc.get(pos) {
        Some(b'B') => ("byte".to_string(), pos + 1),
        Some(b'C') => ("char".to_string(), pos + 1),
        Some(b'D') => ("double".to_string(), pos + 1),
        Some(b'F') => ("float".to_string(), pos + 1),
        Some(b'I') => ("int".to_string(), pos + 1),
        Some(b'J') => ("long".to_string(), pos + 1),
        Some(b'S') => ("short".to_string(), pos + 1),
        Some(b'Z') => ("boolean".to_string(), pos + 1),
        Some(b'V') => ("void".to_string(), pos + 1),
        Some(b'L') => {
            let close = desc[pos..]
                .iter()
                .position(|&b| b == b';')
                .map_or(desc.len(), |i| pos + i);
            (
                String::from_utf8_lossy(&desc[pos + 1..close]).replace('/', "."),
                close + 1,
            )
        }
        _ => ("?".to_string(), desc.len()),
    };
    (format!("{base}{}", "[]".repeat(dims)), end)
}

/// A method descriptor as (parameter types, return type).
fn method_types(desc: &str) -> (Vec<String>, String) {
    let bytes = desc.as_bytes();
    let mut params = Vec::new();
    let mut pos = 1;
    while pos < bytes.len() && bytes[pos] != b')' {
        let (t, end) = field_type(bytes, pos);
        params.push(t);
        if end <= pos {
            break;
        }
        pos = end;
    }
    let (ret, _) = field_type(bytes, pos + 1);
    (params, ret)
}

fn modifiers(access: u16, table: &[(u16, &str)]) -> String {
    let mut out = String::new();
    for (bit, word) in table {
        if access & bit != 0 {
            out.push_str(word);
            out.push(' ');
        }
    }
    out
}

const CLASS_FLAGS: &[(u16, &str)] = &[
    (0x0001, "public"),
    (0x0010, "final"),
    (0x0400, "abstract"),
    (0x1000, "synthetic"),
];
const FIELD_FLAGS: &[(u16, &str)] = &[
    (0x0001, "public"),
    (0x0002, "private"),
    (0x0004, "protected"),
    (0x0008, "static"),
    (0x0010, "final"),
    (0x0040, "volatile"),
    (0x0080, "transient"),
    (0x1000, "synthetic"),
];
const METHOD_FLAGS: &[(u16, &str)] = &[
    (0x0001, "public"),
    (0x0002, "private"),
    (0x0004, "protected"),
    (0x0008, "static"),
    (0x0010, "final"),
    (0x0020, "synchronized"),
    (0x0040, "bridge"),
    (0x0100, "native"),
    (0x0400, "abstract"),
    (0x0800, "strictfp"),
    (0x1000, "synthetic"),
];

/// The Java release of a class file's major version.
fn java_release(major: u16) -> String {
    match major {
        45 => "1.1".into(),
        46..=48 => format!("1.{}", major - 44),
        49..=255 => (major - 44).to_string(),
        _ => "?".into(),
    }
}

impl Class<'_> {
    fn utf8(&self, ix: u16) -> &str {
        match self.pool.get(ix as usize) {
            Some(Constant::Utf8(s)) => s,
            _ => "",
        }
    }

    fn class_name(&self, ix: u16) -> String {
        match self.pool.get(ix as usize) {
            Some(Constant::Class(name)) => self.utf8(*name).to_string(),
            _ => String::new(),
        }
    }

    fn dotted(&self, ix: u16) -> String {
        self.class_name(ix).replace('/', ".")
    }

    fn name_and_type(&self, ix: u16) -> String {
        match self.pool.get(ix as usize) {
            Some(Constant::NameAndType(n, t)) => format!("{}:{}", self.utf8(*n), self.utf8(*t)),
            _ => String::new(),
        }
    }

    /// A constant as `javap` comments it (`Method java/io/PrintStream.println:(I)V`).
    fn constant(&self, ix: u16) -> String {
        match self.pool.get(ix as usize) {
            Some(Constant::Utf8(s)) => format!("Utf8 {}", crate::out::quote(s.as_bytes())),
            Some(Constant::Integer(v)) => format!("int {v}"),
            Some(Constant::Float(v)) => format!("float {v}f"),
            Some(Constant::Long(v)) => format!("long {v}l"),
            Some(Constant::Double(v)) => format!("double {v}d"),
            Some(Constant::Class(n)) => format!("class {}", self.utf8(*n)),
            Some(Constant::String(s)) => {
                format!("String {}", crate::out::quote(self.utf8(*s).as_bytes()))
            }
            Some(Constant::Field(c, nt)) => {
                format!("Field {}.{}", self.class_name(*c), self.name_and_type(*nt))
            }
            Some(Constant::Method(c, nt)) => {
                format!("Method {}.{}", self.class_name(*c), self.name_and_type(*nt))
            }
            Some(Constant::InterfaceMethod(c, nt)) => format!(
                "InterfaceMethod {}.{}",
                self.class_name(*c),
                self.name_and_type(*nt)
            ),
            Some(Constant::NameAndType(..)) => format!("NameAndType {}", self.name_and_type(ix)),
            Some(Constant::MethodHandle(kind, r)) => {
                let kind = match kind {
                    1 => "REF_getField",
                    2 => "REF_getStatic",
                    3 => "REF_putField",
                    4 => "REF_putStatic",
                    5 => "REF_invokeVirtual",
                    6 => "REF_invokeStatic",
                    7 => "REF_invokeSpecial",
                    8 => "REF_newInvokeSpecial",
                    9 => "REF_invokeInterface",
                    _ => "REF_?",
                };
                format!("MethodHandle {kind} {}", self.constant(*r))
            }
            Some(Constant::MethodType(d)) => format!("MethodType {}", self.utf8(*d)),
            Some(Constant::Dynamic(b, nt)) => format!("Dynamic #{b}:{}", self.name_and_type(*nt)),
            Some(Constant::InvokeDynamic(b, nt)) => {
                format!("InvokeDynamic #{b}:{}", self.name_and_type(*nt))
            }
            Some(Constant::Module(n)) => format!("Module {}", self.utf8(*n)),
            Some(Constant::Package(n)) => format!("Package {}", self.utf8(*n)),
            _ => format!("#{ix}"),
        }
    }

    fn attribute<'b>(attributes: &'b [Attribute<'_>], name: &str) -> Option<&'b [u8]> {
        attributes.iter().find(|a| a.name == name).map(|a| a.data)
    }

    fn listing(&self) -> String {
        let mut out = String::new();
        if let Some(data) = Self::attribute(&self.attributes, "SourceFile") {
            let mut r = Reader::new(data);
            if let Some(ix) = r.u2() {
                let _ = writeln!(out, "// Compiled from \"{}\"", self.utf8(ix));
            }
        }
        let _ = writeln!(
            out,
            "// class file version {}.{} (Java {})",
            self.major,
            self.minor,
            java_release(self.major)
        );
        let is_interface = self.access & 0x0200 != 0;
        let mut head = modifiers(
            if is_interface {
                self.access & !0x0400
            } else {
                self.access
            },
            CLASS_FLAGS,
        );
        head.push_str(if self.access & 0x2000 != 0 {
            "@interface "
        } else if is_interface {
            "interface "
        } else if self.access & 0x4000 != 0 {
            "enum "
        } else if self.access & 0x8000 != 0 {
            "module "
        } else if Self::attribute(&self.attributes, "Record").is_some() {
            "record "
        } else {
            "class "
        });
        head.push_str(&self.dotted(self.this));
        let super_name = self.dotted(self.super_class);
        if !super_name.is_empty() && super_name != "java.lang.Object" {
            let _ = write!(head, " extends {super_name}");
        }
        if !self.interfaces.is_empty() {
            let names: Vec<String> = self.interfaces.iter().map(|i| self.dotted(*i)).collect();
            let _ = write!(
                head,
                " {} {}",
                if is_interface {
                    "extends"
                } else {
                    "implements"
                },
                names.join(", ")
            );
        }
        self.annotations(&mut out, &self.attributes, "");
        let _ = writeln!(out, "{head} {{");
        self.signature(&mut out, &self.attributes, "  ");
        if let Some(data) = Self::attribute(&self.attributes, "PermittedSubclasses") {
            let _ = writeln!(out, "  // permits {}", self.class_list(data).join(", "));
        }
        if let Some(data) = Self::attribute(&self.attributes, "NestMembers") {
            let _ = writeln!(
                out,
                "  // nest members {}",
                self.class_list(data).join(", ")
            );
        }
        if let Some(data) = Self::attribute(&self.attributes, "InnerClasses") {
            self.inner_classes(&mut out, data);
        }
        for field in &self.fields {
            out.push('\n');
            self.annotations(&mut out, &field.attributes, "  ");
            let (ty, _) = field_type(field.descriptor.as_bytes(), 0);
            let _ = writeln!(
                out,
                "  {}{ty} {};",
                modifiers(field.access, FIELD_FLAGS),
                field.name
            );
            self.signature(&mut out, &field.attributes, "    ");
            if let Some(data) = Self::attribute(&field.attributes, "ConstantValue") {
                let mut r = Reader::new(data);
                if let Some(ix) = r.u2() {
                    let _ = writeln!(out, "    // ConstantValue: {}", self.constant(ix));
                }
            }
        }
        for method in &self.methods {
            out.push('\n');
            self.method(&mut out, method, is_interface);
        }
        out.push_str("}\n");
        if let Some(data) = Self::attribute(&self.attributes, "BootstrapMethods") {
            self.bootstrap_methods(&mut out, data);
        }
        out
    }

    fn class_list(&self, data: &[u8]) -> Vec<String> {
        let mut r = Reader::new(data);
        let n = r.u2().unwrap_or(0);
        (0..n)
            .filter_map(|_| r.u2())
            .map(|i| self.dotted(i))
            .collect()
    }

    fn signature(&self, out: &mut String, attributes: &[Attribute<'_>], indent: &str) {
        if let Some(data) = Self::attribute(attributes, "Signature") {
            let mut r = Reader::new(data);
            if let Some(ix) = r.u2() {
                let _ = writeln!(out, "{indent}// Signature: {}", self.utf8(ix));
            }
        }
    }

    fn inner_classes(&self, out: &mut String, data: &[u8]) {
        let mut r = Reader::new(data);
        let n = r.u2().unwrap_or(0);
        for _ in 0..n {
            let (Some(inner), Some(_outer), Some(name), Some(access)) =
                (r.u2(), r.u2(), r.u2(), r.u2())
            else {
                return;
            };
            let simple = if name == 0 {
                "(anonymous)".to_string()
            } else {
                self.utf8(name).to_string()
            };
            let _ = writeln!(
                out,
                "  // inner class {}{} = {}",
                modifiers(access & 0x0f1f, FIELD_FLAGS),
                simple,
                self.dotted(inner)
            );
        }
    }

    fn method(&self, out: &mut String, method: &Member<'_>, in_interface: bool) {
        self.annotations(out, &method.attributes, "  ");
        let (params, ret) = method_types(&method.descriptor);
        let mut params = params;
        if method.access & 0x0080 != 0
            && let Some(last) = params.last_mut()
            && last.ends_with("[]")
        {
            last.truncate(last.len() - 2);
            last.push_str("...");
        }
        let mut access = method.access;
        if in_interface && access & 0x0400 != 0 {
            access &= !0x0400;
        }
        let mut line = format!("  {}", modifiers(access, METHOD_FLAGS));
        if in_interface && method.access & (0x0400 | 0x0008 | 0x0002) == 0 {
            line.push_str("default ");
        }
        let this = self.dotted(self.this);
        match method.name.as_str() {
            "<init>" => line.push_str(&this),
            "<clinit>" => line = "  static {}".to_string(),
            name => {
                let _ = write!(line, "{ret} {name}");
            }
        }
        if method.name != "<clinit>" {
            let _ = write!(line, "({})", params.join(", "));
        }
        if let Some(data) = Self::attribute(&method.attributes, "Exceptions") {
            let names = self.class_list(data);
            if !names.is_empty() {
                let _ = write!(line, " throws {}", names.join(", "));
            }
        }
        let _ = writeln!(out, "{line};");
        let _ = writeln!(out, "    descriptor: {}", method.descriptor);
        self.signature(out, &method.attributes, "    ");
        if let Some(data) = Self::attribute(&method.attributes, "Code") {
            self.code(out, data);
        }
    }

    fn code(&self, out: &mut String, data: &[u8]) {
        let mut r = Reader::new(data);
        let (Some(stack), Some(locals), Some(len)) = (r.u2(), r.u2(), r.u4()) else {
            return;
        };
        let Some(code) = r.bytes(len as usize) else {
            return;
        };
        let _ = writeln!(out, "    Code: stack={stack}, locals={locals}");
        let Some(instructions) = decode(code) else {
            let _ = writeln!(out, "      (unreadable bytecode)");
            return;
        };
        let mut handlers = Vec::new();
        if let Some(n) = r.u2() {
            for _ in 0..n {
                let (Some(from), Some(to), Some(target), Some(ty)) =
                    (r.u2(), r.u2(), r.u2(), r.u2())
                else {
                    break;
                };
                handlers.push((from as u32, to as u32, target as u32, ty));
            }
        }
        // labels for every jump target and handler bound, numbered in order
        let mut targets: BTreeMap<u32, usize> = BTreeMap::new();
        for ins in &instructions {
            for t in &ins.targets {
                targets.insert(*t, 0);
            }
        }
        for (from, to, target, _) in &handlers {
            targets.insert(*from, 0);
            targets.insert(*to, 0);
            targets.insert(*target, 0);
        }
        for (n, v) in targets.values_mut().enumerate() {
            *v = n;
        }
        let label = |offset: u32| {
            targets
                .get(&offset)
                .map_or_else(|| format!("@{offset}"), |n| format!("L{n}"))
        };
        for ins in &instructions {
            if let Some(n) = targets.get(&ins.offset) {
                let _ = writeln!(out, "     L{n}:");
            }
            let mut line = format!("       {}", OPCODES[ins.opcode as usize]);
            match &ins.operand {
                Operand::None => {}
                Operand::Int(v) => {
                    let _ = write!(line, " {v}");
                }
                Operand::Local(v) => {
                    let _ = write!(line, " {v}");
                }
                Operand::Iinc(local, by) => {
                    let _ = write!(line, " {local}, {by}");
                }
                Operand::Constant(ix) => {
                    let _ = write!(line, " {}", self.constant(*ix));
                }
                Operand::Interface(ix, count) => {
                    let _ = write!(line, " {}, {count}", self.constant(*ix));
                }
                Operand::Multi(ix, dims) => {
                    let _ = write!(line, " {}, {dims}", self.constant(*ix));
                }
                Operand::ArrayType(t) => {
                    let name = match t {
                        4 => "boolean",
                        5 => "char",
                        6 => "float",
                        7 => "double",
                        8 => "byte",
                        9 => "short",
                        10 => "int",
                        11 => "long",
                        _ => "?",
                    };
                    let _ = write!(line, " {name}");
                }
                Operand::Branch => {
                    let _ = write!(line, " {}", label(ins.targets[0]));
                }
                Operand::Table { low } => {
                    let _ = write!(
                        line,
                        " {{ // {low} to {}",
                        low + ins.targets.len() as i32 - 2
                    );
                    for (k, t) in ins.targets[1..].iter().enumerate() {
                        let _ = write!(line, "\n         {}: {}", low + k as i32, label(*t));
                    }
                    let _ = write!(
                        line,
                        "\n         default: {}\n       }}",
                        label(ins.targets[0])
                    );
                }
                Operand::Lookup(keys) => {
                    let _ = write!(line, " {{ // {}", keys.len());
                    for (k, t) in keys.iter().zip(&ins.targets[1..]) {
                        let _ = write!(line, "\n         {k}: {}", label(*t));
                    }
                    let _ = write!(
                        line,
                        "\n         default: {}\n       }}",
                        label(ins.targets[0])
                    );
                }
            }
            let _ = writeln!(out, "{line}");
        }
        if let Some(n) = targets.get(&(code.len() as u32)) {
            let _ = writeln!(out, "     L{n}:");
        }
        if !handlers.is_empty() {
            let _ = writeln!(out, "    Exception table:");
            for (from, to, target, ty) in handlers {
                let ty = if ty == 0 {
                    "any".to_string()
                } else {
                    self.class_name(ty)
                };
                let _ = writeln!(
                    out,
                    "      {} to {} goes to {} for {ty}",
                    label(from),
                    label(to),
                    label(target)
                );
            }
        }
    }

    fn bootstrap_methods(&self, out: &mut String, data: &[u8]) {
        let mut r = Reader::new(data);
        let n = r.u2().unwrap_or(0);
        let _ = writeln!(out, "BootstrapMethods:");
        for ix in 0..n {
            let (Some(handle), Some(count)) = (r.u2(), r.u2()) else {
                return;
            };
            let _ = writeln!(out, "  {ix}: {}", self.constant(handle));
            for _ in 0..count {
                if let Some(arg) = r.u2() {
                    let _ = writeln!(out, "      {}", self.constant(arg));
                }
            }
        }
    }

    fn annotations(&self, out: &mut String, attributes: &[Attribute<'_>], indent: &str) {
        for name in ["RuntimeVisibleAnnotations", "RuntimeInvisibleAnnotations"] {
            let Some(data) = Self::attribute(attributes, name) else {
                continue;
            };
            let mut r = Reader::new(data);
            let n = r.u2().unwrap_or(0);
            for _ in 0..n {
                match self.annotation(&mut r) {
                    Some(text) => {
                        let _ = writeln!(out, "{indent}{text}");
                    }
                    None => return,
                }
            }
        }
        if Self::attribute(attributes, "Deprecated").is_some() {
            let _ = writeln!(out, "{indent}// deprecated");
        }
    }

    fn annotation(&self, r: &mut Reader<'_>) -> Option<String> {
        let ty = self.utf8(r.u2()?).to_string();
        let (name, _) = field_type(ty.as_bytes(), 0);
        let pairs = r.u2()?;
        let mut values = Vec::new();
        for _ in 0..pairs {
            let key = self.utf8(r.u2()?).to_string();
            let value = self.element_value(r)?;
            values.push(format!("{key}={value}"));
        }
        // Kotlin's metadata is encoded data, not something to read
        if name == "kotlin.Metadata" {
            return Some("@kotlin.Metadata(…)".into());
        }
        Some(if values.is_empty() {
            format!("@{name}")
        } else {
            format!("@{name}({})", values.join(", "))
        })
    }

    fn element_value(&self, r: &mut Reader<'_>) -> Option<String> {
        let tag = r.u1()?;
        Some(match tag {
            b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' | b's' => {
                let ix = r.u2()?;
                match self.pool.get(ix as usize) {
                    Some(Constant::Utf8(s)) => crate::out::quote(s.as_bytes()),
                    Some(Constant::Integer(v)) if tag == b'Z' => (*v != 0).to_string(),
                    Some(Constant::Integer(v)) if tag == b'C' => {
                        format!("'{}'", char::from_u32(*v as u32).unwrap_or('?'))
                    }
                    Some(Constant::Integer(v)) => v.to_string(),
                    Some(Constant::Long(v)) => format!("{v}L"),
                    Some(Constant::Float(v)) => format!("{v}f"),
                    Some(Constant::Double(v)) => v.to_string(),
                    _ => "?".into(),
                }
            }
            b'e' => {
                let ty = self.utf8(r.u2()?).to_string();
                let constant = self.utf8(r.u2()?).to_string();
                format!("{}.{constant}", field_type(ty.as_bytes(), 0).0)
            }
            b'c' => format!("{}.class", field_type(self.utf8(r.u2()?).as_bytes(), 0).0),
            b'@' => self.annotation(r)?,
            b'[' => {
                let n = r.u2()?;
                let mut items = Vec::new();
                for _ in 0..n {
                    items.push(self.element_value(r)?);
                }
                format!("{{{}}}", items.join(", "))
            }
            _ => return None,
        })
    }
}

enum Operand {
    None,
    Int(i32),
    Local(u16),
    Iinc(u16, i16),
    Constant(u16),
    Interface(u16, u8),
    Multi(u16, u8),
    ArrayType(u8),
    Branch,
    Table { low: i32 },
    Lookup(Vec<i32>),
}

struct Instruction {
    offset: u32,
    opcode: u8,
    operand: Operand,
    /// Jump targets (absolute offsets); a switch's default first.
    targets: Vec<u32>,
}

fn decode(code: &[u8]) -> Option<Vec<Instruction>> {
    let mut r = Reader::new(code);
    let mut out = Vec::new();
    while r.pos < code.len() {
        let offset = r.pos as u32;
        let opcode = r.u1()?;
        let rel = |d: i32| (offset as i64 + d as i64).max(0) as u32;
        let mut targets = Vec::new();
        let operand = match opcode {
            0x10 => Operand::Int(r.i1()? as i32),
            0x11 => Operand::Int(r.i2()? as i32),
            0x12 => Operand::Constant(r.u1()? as u16),
            0x13 | 0x14 => Operand::Constant(r.u2()?),
            0x15..=0x19 | 0x36..=0x3a | 0xa9 => Operand::Local(r.u1()? as u16),
            0x84 => Operand::Iinc(r.u1()? as u16, r.i1()? as i16),
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                targets.push(rel(r.i2()? as i32));
                Operand::Branch
            }
            0xc8 | 0xc9 => {
                targets.push(rel(r.i4()?));
                Operand::Branch
            }
            0xaa => {
                while !r.pos.is_multiple_of(4) {
                    r.u1()?;
                }
                let default = r.i4()?;
                let low = r.i4()?;
                let high = r.i4()?;
                if high < low || (high as i64 - low as i64) > 65_536 {
                    return None;
                }
                targets.push(rel(default));
                for _ in low..=high {
                    targets.push(rel(r.i4()?));
                }
                Operand::Table { low }
            }
            0xab => {
                while !r.pos.is_multiple_of(4) {
                    r.u1()?;
                }
                let default = r.i4()?;
                let n = r.i4()?;
                if !(0..=65_536).contains(&n) {
                    return None;
                }
                targets.push(rel(default));
                let mut keys = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    keys.push(r.i4()?);
                    targets.push(rel(r.i4()?));
                }
                Operand::Lookup(keys)
            }
            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => Operand::Constant(r.u2()?),
            0xb9 => {
                let ix = r.u2()?;
                let count = r.u1()?;
                r.u1()?;
                Operand::Interface(ix, count)
            }
            0xba => {
                let ix = r.u2()?;
                r.u2()?;
                Operand::Constant(ix)
            }
            0xbc => Operand::ArrayType(r.u1()?),
            0xc5 => {
                let ix = r.u2()?;
                Operand::Multi(ix, r.u1()?)
            }
            0xc4 => {
                let op = r.u1()?;
                let local = r.u2()?;
                if op == 0x84 {
                    let by = r.i2()?;
                    out.push(Instruction {
                        offset,
                        opcode: op,
                        operand: Operand::Iinc(local, by),
                        targets,
                    });
                } else {
                    out.push(Instruction {
                        offset,
                        opcode: op,
                        operand: Operand::Local(local),
                        targets,
                    });
                }
                continue;
            }
            op if (op as usize) < OPCODES.len() => Operand::None,
            _ => return None,
        };
        out.push(Instruction {
            offset,
            opcode,
            operand,
            targets,
        });
    }
    Some(out)
}

/// JVM opcode mnemonics by value (`0x00`..`0xca`).
const OPCODES: [&str; 203] = [
    "nop",
    "aconst_null",
    "iconst_m1",
    "iconst_0",
    "iconst_1",
    "iconst_2",
    "iconst_3",
    "iconst_4",
    "iconst_5",
    "lconst_0",
    "lconst_1",
    "fconst_0",
    "fconst_1",
    "fconst_2",
    "dconst_0",
    "dconst_1",
    "bipush",
    "sipush",
    "ldc",
    "ldc_w",
    "ldc2_w",
    "iload",
    "lload",
    "fload",
    "dload",
    "aload",
    "iload_0",
    "iload_1",
    "iload_2",
    "iload_3",
    "lload_0",
    "lload_1",
    "lload_2",
    "lload_3",
    "fload_0",
    "fload_1",
    "fload_2",
    "fload_3",
    "dload_0",
    "dload_1",
    "dload_2",
    "dload_3",
    "aload_0",
    "aload_1",
    "aload_2",
    "aload_3",
    "iaload",
    "laload",
    "faload",
    "daload",
    "aaload",
    "baload",
    "caload",
    "saload",
    "istore",
    "lstore",
    "fstore",
    "dstore",
    "astore",
    "istore_0",
    "istore_1",
    "istore_2",
    "istore_3",
    "lstore_0",
    "lstore_1",
    "lstore_2",
    "lstore_3",
    "fstore_0",
    "fstore_1",
    "fstore_2",
    "fstore_3",
    "dstore_0",
    "dstore_1",
    "dstore_2",
    "dstore_3",
    "astore_0",
    "astore_1",
    "astore_2",
    "astore_3",
    "iastore",
    "lastore",
    "fastore",
    "dastore",
    "aastore",
    "bastore",
    "castore",
    "sastore",
    "pop",
    "pop2",
    "dup",
    "dup_x1",
    "dup_x2",
    "dup2",
    "dup2_x1",
    "dup2_x2",
    "swap",
    "iadd",
    "ladd",
    "fadd",
    "dadd",
    "isub",
    "lsub",
    "fsub",
    "dsub",
    "imul",
    "lmul",
    "fmul",
    "dmul",
    "idiv",
    "ldiv",
    "fdiv",
    "ddiv",
    "irem",
    "lrem",
    "frem",
    "drem",
    "ineg",
    "lneg",
    "fneg",
    "dneg",
    "ishl",
    "lshl",
    "ishr",
    "lshr",
    "iushr",
    "lushr",
    "iand",
    "land",
    "ior",
    "lor",
    "ixor",
    "lxor",
    "iinc",
    "i2l",
    "i2f",
    "i2d",
    "l2i",
    "l2f",
    "l2d",
    "f2i",
    "f2l",
    "f2d",
    "d2i",
    "d2l",
    "d2f",
    "i2b",
    "i2c",
    "i2s",
    "lcmp",
    "fcmpl",
    "fcmpg",
    "dcmpl",
    "dcmpg",
    "ifeq",
    "ifne",
    "iflt",
    "ifge",
    "ifgt",
    "ifle",
    "if_icmpeq",
    "if_icmpne",
    "if_icmplt",
    "if_icmpge",
    "if_icmpgt",
    "if_icmple",
    "if_acmpeq",
    "if_acmpne",
    "goto",
    "jsr",
    "ret",
    "tableswitch",
    "lookupswitch",
    "ireturn",
    "lreturn",
    "freturn",
    "dreturn",
    "areturn",
    "return",
    "getstatic",
    "putstatic",
    "getfield",
    "putfield",
    "invokevirtual",
    "invokespecial",
    "invokestatic",
    "invokeinterface",
    "invokedynamic",
    "new",
    "newarray",
    "anewarray",
    "arraylength",
    "athrow",
    "checkcast",
    "instanceof",
    "monitorenter",
    "monitorexit",
    "wide",
    "multianewarray",
    "ifnull",
    "ifnonnull",
    "goto_w",
    "jsr_w",
    "breakpoint",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-assembled class:
    /// `public class Hi { public static int f(int x) { return x > 0 ? 1 : 2; } }`
    fn sample() -> Vec<u8> {
        let mut b: Vec<u8> = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61];
        let utf8 = |s: &str| {
            let mut v = vec![1u8];
            v.extend((s.len() as u16).to_be_bytes());
            v.extend(s.as_bytes());
            v
        };
        let pool: Vec<Vec<u8>> = vec![
            utf8("Hi"),               // 1
            vec![7, 0, 1],            // 2 class Hi
            utf8("java/lang/Object"), // 3
            vec![7, 0, 3],            // 4
            utf8("f"),                // 5
            utf8("(I)I"),             // 6
            utf8("Code"),             // 7
            utf8("SourceFile"),       // 8
            utf8("Hi.java"),          // 9
        ];
        b.extend(((pool.len() + 1) as u16).to_be_bytes());
        for p in pool {
            b.extend(p);
        }
        b.extend(0x0021u16.to_be_bytes()); // public super
        b.extend(2u16.to_be_bytes());
        b.extend(4u16.to_be_bytes());
        b.extend(0u16.to_be_bytes()); // interfaces
        b.extend(0u16.to_be_bytes()); // fields
        b.extend(1u16.to_be_bytes()); // methods
        b.extend(0x0009u16.to_be_bytes());
        b.extend(5u16.to_be_bytes());
        b.extend(6u16.to_be_bytes());
        b.extend(1u16.to_be_bytes());
        // iload_0; ifle +7; iconst_1; goto +4; iconst_2; ireturn
        let code: &[u8] = &[0x1a, 0x9e, 0x00, 0x07, 0x04, 0xa7, 0x00, 0x04, 0x05, 0xac];
        let mut attr = Vec::new();
        attr.extend(1u16.to_be_bytes());
        attr.extend(1u16.to_be_bytes());
        attr.extend((code.len() as u32).to_be_bytes());
        attr.extend(code);
        attr.extend(0u16.to_be_bytes());
        attr.extend(0u16.to_be_bytes());
        b.extend(7u16.to_be_bytes());
        b.extend((attr.len() as u32).to_be_bytes());
        b.extend(attr);
        b.extend(1u16.to_be_bytes()); // class attributes
        b.extend(8u16.to_be_bytes());
        b.extend(2u32.to_be_bytes());
        b.extend(9u16.to_be_bytes());
        b
    }

    #[test]
    fn lists_a_class() {
        let text = to_text(&sample()).unwrap();
        assert!(text.contains("// Compiled from \"Hi.java\""), "{text}");
        assert!(text.contains("public class Hi {"), "{text}");
        assert!(text.contains("public static int f(int);"), "{text}");
        assert!(text.contains("ifle L0"), "{text}");
        assert!(text.contains("goto L1"), "{text}");
        assert!(text.contains("     L0:\n       iconst_2"), "{text}");
        assert!(!text.contains("0: "), "offsets are left out: {text}");
    }

    #[test]
    fn descriptors_read_as_java_types() {
        assert_eq!(
            method_types("([Ljava/lang/String;JZ)V"),
            (
                vec!["java.lang.String[]".into(), "long".into(), "boolean".into()],
                "void".into()
            )
        );
    }

    #[test]
    fn garbage_is_refused() {
        assert!(to_text(b"\xca\xfe\xba\xbe\0\0\0\x34\0").is_err());
        assert!(to_text(b"hello").is_err());
    }
}
