//! A minimal pickle reader (protocols 0 to 5) for `torch.save` archives.
//! Nothing is executed: the few torch rebuild functions are recognized by
//! name, collections become dicts and lists, every other global becomes an
//! opaque object that keeps its `BUILD` state.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use corvene_models::tensors::DType;

use crate::Error;
use crate::source::bad;

/// A storage referenced by a persistent id.
#[derive(Debug)]
pub(crate) struct StorageRef {
    pub dtype: DType,
    pub key: String,
}

/// A tensor rebuilt from a storage.
#[derive(Debug)]
pub(crate) struct TensorRef {
    pub storage: Rc<StorageRef>,
    pub offset: u64,
    pub size: Vec<u64>,
    pub stride: Vec<u64>,
}

/// A pickled value.
#[derive(Clone, Debug)]
pub(crate) enum V {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes,
    Tuple(Rc<Vec<V>>),
    List(Rc<RefCell<Vec<V>>>),
    Dict(Rc<RefCell<Vec<(V, V)>>>),
    Global(Rc<str>),
    Storage(Rc<StorageRef>),
    Tensor(Rc<TensorRef>),
    /// An instance of a class not understood; `state` is what `BUILD` set.
    Object {
        class: Rc<str>,
        state: Rc<RefCell<V>>,
    },
}

impl V {
    fn int(&self) -> Option<i64> {
        match self {
            V::Int(i) => Some(*i),
            V::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    fn items(&self) -> Option<Vec<V>> {
        match self {
            V::Tuple(t) => Some(t.as_ref().clone()),
            V::List(l) => Some(l.borrow().clone()),
            _ => None,
        }
    }

    fn object(class: &str) -> V {
        V::Object {
            class: class.into(),
            state: Rc::new(RefCell::new(V::None)),
        }
    }
}

// Deeply nested containers would overflow the stack when dropped
// recursively; children are moved out onto a heap stack instead.
impl Drop for V {
    fn drop(&mut self) {
        fn take(v: &mut V, out: &mut Vec<V>) {
            match v {
                V::Tuple(t) => {
                    if let Some(items) = Rc::get_mut(t) {
                        out.append(items);
                    }
                }
                V::List(l) => {
                    if Rc::strong_count(l) == 1 {
                        out.append(&mut l.borrow_mut());
                    }
                }
                V::Dict(d) => {
                    if Rc::strong_count(d) == 1 {
                        for (k, v) in d.borrow_mut().drain(..) {
                            out.push(k);
                            out.push(v);
                        }
                    }
                }
                V::Object { state, .. } if Rc::strong_count(state) == 1 => {
                    out.push(std::mem::replace(&mut *state.borrow_mut(), V::None));
                }
                _ => {}
            }
        }
        let mut pending = Vec::new();
        take(self, &mut pending);
        while let Some(mut v) = pending.pop() {
            take(&mut v, &mut pending);
        }
    }
}

fn storage_dtype(name: &str) -> Option<DType> {
    let n = name.rsplit('.').next().unwrap_or(name);
    Some(match n {
        "FloatStorage" => DType::F32,
        "DoubleStorage" => DType::F64,
        "HalfStorage" => DType::F16,
        "BFloat16Storage" => DType::BF16,
        "LongStorage" => DType::I64,
        "IntStorage" => DType::I32,
        "ShortStorage" => DType::I16,
        "CharStorage" => DType::I8,
        "ByteStorage" => DType::U8,
        "BoolStorage" => DType::Bool,
        "UInt16Storage" => DType::U16,
        "UInt32Storage" => DType::U32,
        "UInt64Storage" => DType::U64,
        "ComplexFloatStorage" => DType::C64,
        "ComplexDoubleStorage" => DType::C128,
        "Float8_e4m3fnStorage" => DType::F8E4M3,
        "Float8_e5m2Storage" => DType::F8E5M2,
        other if other.ends_with("Storage") => {
            DType::Other(other.trim_end_matches("Storage").to_string())
        }
        _ => return None,
    })
}

fn dims(v: &V) -> Option<Vec<u64>> {
    v.items()?
        .iter()
        .map(|d| d.int().and_then(|i| u64::try_from(i).ok()))
        .collect()
}

/// Applies a callable global to its arguments, without running anything.
fn call(func: &V, args: &V, depth: u32) -> V {
    let name = match func {
        V::Global(n) => n.clone(),
        V::Object { class, .. } => class.clone(),
        _ => return V::object("call"),
    };
    let args = args.items().unwrap_or_default();
    match &*name {
        "collections.OrderedDict" | "builtins.dict" | "__builtin__.dict" => {
            let mut pairs = Vec::new();
            if let Some(items) = args.first().and_then(|a| a.items()) {
                for it in items {
                    if let Some(kv) = it.items()
                        && kv.len() == 2
                    {
                        pairs.push((kv[0].clone(), kv[1].clone()));
                    }
                }
            }
            V::Dict(Rc::new(RefCell::new(pairs)))
        }
        "builtins.list" | "__builtin__.list" => V::List(Rc::new(RefCell::new(
            args.first().and_then(|a| a.items()).unwrap_or_default(),
        ))),
        "torch._utils._rebuild_tensor_v2"
        | "torch._utils._rebuild_tensor"
        | "torch._utils._rebuild_tensor_v3" => {
            let storage = match args.first() {
                Some(V::Storage(s)) => s.clone(),
                _ => return V::object("tensor without storage"),
            };
            let offset = args.get(1).and_then(|o| o.int()).unwrap_or(0).max(0) as u64;
            let size = args.get(2).and_then(dims).unwrap_or_default();
            let stride = args.get(3).and_then(dims).unwrap_or_default();
            V::Tensor(Rc::new(TensorRef {
                storage,
                offset,
                size,
                stride,
            }))
        }
        "torch._utils._rebuild_parameter" | "torch._utils._rebuild_parameter_with_state" => {
            args.first().cloned().unwrap_or(V::None)
        }
        "torch._tensor._rebuild_from_type_v2" | "torch._tensor._rebuild_from_type" => {
            if depth > 8 {
                return V::object("rebuild");
            }
            match (args.first(), args.get(2)) {
                (Some(f), Some(a)) => call(f, a, depth + 1),
                _ => V::object("rebuild"),
            }
        }
        "torch.Size" => args.first().cloned().unwrap_or(V::None),
        _ => V::object(&name),
    }
}

/// Runs the pickle and returns the unpickled root value.
pub(crate) fn load(data: &[u8]) -> Result<V, Error> {
    let mut stack: Vec<V> = Vec::new();
    let mut marks: Vec<usize> = Vec::new();
    let mut memo: HashMap<u64, V> = HashMap::new();
    let mut i = 0usize;

    macro_rules! take {
        ($n:expr) => {{
            let n: usize = $n;
            let end = i
                .checked_add(n)
                .filter(|e| *e <= data.len())
                .ok_or_else(|| bad("pickle truncated"))?;
            let s = &data[i..end];
            i = end;
            s
        }};
    }
    macro_rules! pop {
        () => {
            stack.pop().ok_or_else(|| bad("pickle stack underflow"))?
        };
    }
    macro_rules! line {
        () => {{
            let rest = &data[i..];
            let n = rest
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| bad("pickle truncated"))?;
            let s = String::from_utf8_lossy(&rest[..n]).into_owned();
            i += n + 1;
            s
        }};
    }
    macro_rules! pop_mark {
        () => {{
            let m = marks.pop().ok_or_else(|| bad("pickle mark missing"))?;
            if m > stack.len() {
                return Err(bad("pickle mark past the stack"));
            }
            stack.split_off(m)
        }};
    }
    fn le(b: &[u8]) -> u64 {
        b.iter().rev().fold(0u64, |a, x| (a << 8) | *x as u64)
    }
    fn long(b: &[u8]) -> V {
        if b.len() <= 8 {
            let mut v = le(b);
            if b.last().is_some_and(|x| x & 0x80 != 0) && b.len() < 8 {
                v |= u64::MAX << (8 * b.len());
            }
            V::Int(v as i64)
        } else {
            V::Object {
                class: "int".into(),
                state: Rc::new(RefCell::new(V::None)),
            }
        }
    }
    fn text(s: &[u8]) -> V {
        V::Str(String::from_utf8_lossy(s).as_ref().into())
    }

    loop {
        let op = *data.get(i).ok_or_else(|| bad("pickle has no STOP"))?;
        i += 1;
        match op {
            0x80 => {
                take!(1);
            }
            0x95 => {
                take!(8);
            }
            b'.' => return stack.pop().ok_or_else(|| bad("pickle stack empty at STOP")),
            b'(' => marks.push(stack.len()),
            b'0' => {
                pop!();
            }
            b'1' => {
                let _ = pop_mark!();
            }
            b'2' => {
                let top = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| bad("pickle stack underflow"))?;
                stack.push(top);
            }
            b')' => stack.push(V::Tuple(Rc::new(Vec::new()))),
            b't' => {
                let items = pop_mark!();
                stack.push(V::Tuple(Rc::new(items)));
            }
            0x85..=0x87 => {
                let n = (op - 0x84) as usize;
                if stack.len() < n {
                    return Err(bad("pickle stack underflow"));
                }
                let items = stack.split_off(stack.len() - n);
                stack.push(V::Tuple(Rc::new(items)));
            }
            b']' => stack.push(V::List(Rc::new(RefCell::new(Vec::new())))),
            b'l' => {
                let items = pop_mark!();
                stack.push(V::List(Rc::new(RefCell::new(items))));
            }
            b'a' => {
                let v = pop!();
                if let Some(V::List(l)) = stack.last() {
                    l.borrow_mut().push(v);
                }
            }
            b'e' => {
                let items = pop_mark!();
                if let Some(V::List(l)) = stack.last() {
                    l.borrow_mut().extend(items);
                }
            }
            b'}' => stack.push(V::Dict(Rc::new(RefCell::new(Vec::new())))),
            b'd' => {
                let items = pop_mark!();
                let pairs = items
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| (p[0].clone(), p[1].clone()))
                    .collect();
                stack.push(V::Dict(Rc::new(RefCell::new(pairs))));
            }
            b's' => {
                let v = pop!();
                let k = pop!();
                if let Some(V::Dict(d)) = stack.last() {
                    d.borrow_mut().push((k, v));
                }
            }
            b'u' => {
                let items = pop_mark!();
                if let Some(V::Dict(d)) = stack.last() {
                    d.borrow_mut().extend(
                        items
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|p| (p[0].clone(), p[1].clone())),
                    );
                }
            }
            0x8f => stack.push(V::List(Rc::new(RefCell::new(Vec::new())))),
            0x90 => {
                let items = pop_mark!();
                if let Some(V::List(l)) = stack.last() {
                    l.borrow_mut().extend(items);
                }
            }
            0x91 => {
                let items = pop_mark!();
                stack.push(V::List(Rc::new(RefCell::new(items))));
            }
            b'N' => stack.push(V::None),
            0x88 => stack.push(V::Bool(true)),
            0x89 => stack.push(V::Bool(false)),
            b'I' => {
                let s = line!();
                let v = match s.trim() {
                    "01" => V::Bool(true),
                    "00" => V::Bool(false),
                    t => V::Int(t.parse().map_err(|_| bad("pickle INT"))?),
                };
                stack.push(v);
            }
            b'L' => {
                let s = line!();
                stack.push(
                    s.trim()
                        .trim_end_matches('L')
                        .parse::<i64>()
                        .map(V::Int)
                        .unwrap_or_else(|_| V::object("int")),
                );
            }
            b'J' => {
                let b = take!(4);
                stack.push(V::Int(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64));
            }
            b'K' => {
                let b = take!(1);
                stack.push(V::Int(b[0] as i64));
            }
            b'M' => {
                let b = take!(2);
                stack.push(V::Int(u16::from_le_bytes([b[0], b[1]]) as i64));
            }
            0x8a => {
                let n = take!(1)[0] as usize;
                let b = take!(n);
                stack.push(long(b));
            }
            0x8b => {
                let n = le(take!(4)) as usize;
                let b = take!(n);
                stack.push(long(b));
            }
            b'F' => {
                let s = line!();
                stack.push(V::Float(s.trim().parse().unwrap_or(f64::NAN)));
            }
            b'G' => {
                let b = take!(8);
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                stack.push(V::Float(f64::from_be_bytes(a)));
            }
            b'S' => {
                let s = line!();
                let t = s.trim();
                let t = t
                    .strip_prefix('\'')
                    .and_then(|t| t.strip_suffix('\''))
                    .or_else(|| t.strip_prefix('"').and_then(|t| t.strip_suffix('"')))
                    .unwrap_or(t);
                stack.push(V::Str(t.into()));
            }
            b'V' => {
                let s = line!();
                stack.push(V::Str(s.as_str().into()));
            }
            b'T' | b'X' => {
                let n = le(take!(4)) as usize;
                let b = take!(n);
                stack.push(text(b));
            }
            b'U' | 0x8c => {
                let n = take!(1)[0] as usize;
                let b = take!(n);
                stack.push(text(b));
            }
            0x8d => {
                let n = usize::try_from(le(take!(8))).map_err(|_| bad("pickle string too long"))?;
                let b = take!(n);
                stack.push(text(b));
            }
            b'B' => {
                let n = le(take!(4)) as usize;
                take!(n);
                stack.push(V::Bytes);
            }
            b'C' => {
                let n = take!(1)[0] as usize;
                take!(n);
                stack.push(V::Bytes);
            }
            0x8e | 0x96 => {
                let n = usize::try_from(le(take!(8))).map_err(|_| bad("pickle bytes too long"))?;
                take!(n);
                stack.push(V::Bytes);
            }
            b'c' => {
                let module = line!();
                let name = line!();
                stack.push(V::Global(format!("{module}.{name}").into()));
            }
            0x93 => {
                let name = pop!();
                let module = pop!();
                let s = |v: &V| match v {
                    V::Str(s) => s.to_string(),
                    _ => "?".into(),
                };
                stack.push(V::Global(format!("{}.{}", s(&module), s(&name)).into()));
            }
            b'R' => {
                let args = pop!();
                let func = pop!();
                stack.push(call(&func, &args, 0));
            }
            0x81 => {
                let args = pop!();
                let cls = pop!();
                stack.push(call(&cls, &args, 0));
            }
            0x92 => {
                let _kwargs = pop!();
                let args = pop!();
                let cls = pop!();
                stack.push(call(&cls, &args, 0));
            }
            b'o' => {
                let mut items = pop_mark!();
                if items.is_empty() {
                    return Err(bad("pickle OBJ without class"));
                }
                let cls = items.remove(0);
                stack.push(call(&cls, &V::Tuple(Rc::new(items)), 0));
            }
            b'i' => {
                let module = line!();
                let name = line!();
                let _ = pop_mark!();
                stack.push(V::object(&format!("{module}.{name}")));
            }
            b'b' => {
                let state = pop!();
                // A dict's state is its attributes (`_metadata` on state
                // dicts): its own items stay.
                if let Some(V::Object { state: s, .. }) = stack.last() {
                    *s.borrow_mut() = state;
                }
            }
            b'p' => {
                let n = line!();
                let k = n.trim().parse::<u64>().map_err(|_| bad("pickle PUT"))?;
                let top = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| bad("pickle stack underflow"))?;
                memo.insert(k, top);
            }
            b'q' => {
                let k = take!(1)[0] as u64;
                let top = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| bad("pickle stack underflow"))?;
                memo.insert(k, top);
            }
            b'r' => {
                let k = le(take!(4));
                let top = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| bad("pickle stack underflow"))?;
                memo.insert(k, top);
            }
            0x94 => {
                let k = memo.len() as u64;
                let top = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| bad("pickle stack underflow"))?;
                memo.insert(k, top);
            }
            b'g' => {
                let n = line!();
                let k = n.trim().parse::<u64>().map_err(|_| bad("pickle GET"))?;
                stack.push(
                    memo.get(&k)
                        .cloned()
                        .ok_or_else(|| bad("pickle memo miss"))?,
                );
            }
            b'h' => {
                let k = take!(1)[0] as u64;
                stack.push(
                    memo.get(&k)
                        .cloned()
                        .ok_or_else(|| bad("pickle memo miss"))?,
                );
            }
            b'j' => {
                let k = le(take!(4));
                stack.push(
                    memo.get(&k)
                        .cloned()
                        .ok_or_else(|| bad("pickle memo miss"))?,
                );
            }
            b'P' => {
                let _ = line!();
                stack.push(V::object("persistent"));
            }
            b'Q' => {
                let pid = pop!();
                stack.push(persistent(&pid));
            }
            0x82 => {
                take!(1);
                stack.push(V::object("extension"));
            }
            0x83 => {
                take!(2);
                stack.push(V::object("extension"));
            }
            0x84 => {
                take!(4);
                stack.push(V::object("extension"));
            }
            0x97 => stack.push(V::Bytes),
            0x98 => {}
            other => return Err(bad(format!("unknown pickle opcode 0x{other:02x}"))),
        }
    }
}

/// `('storage', torch.FloatStorage, key, location, numel)`.
fn persistent(pid: &V) -> V {
    let Some(items) = pid.items() else {
        return V::object("persistent");
    };
    let mut dtype = None;
    for it in &items {
        if let V::Global(g) = it
            && let Some(d) = storage_dtype(g)
        {
            dtype = Some(d);
            break;
        }
    }
    let key = match items.get(2) {
        Some(V::Str(s)) => s.to_string(),
        Some(V::Int(i)) => i.to_string(),
        _ => return V::object("persistent"),
    };
    match dtype {
        Some(dtype) => V::Storage(Rc::new(StorageRef { dtype, key })),
        None => V::object("persistent"),
    }
}
