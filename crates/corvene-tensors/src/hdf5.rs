//! HDF5 (`.h5`, `.hdf5`) and Keras 3 `.keras` zips holding
//! `model.weights.h5`: superblocks v0 to v3, object headers v1 and v2,
//! symbol table groups (v1 B-tree + local heap), compact links, dense
//! links and attributes (fractal heap blocks scanned in order), datasets
//! (contiguous, compact, chunked with v1 B-tree, single chunk, implicit
//! and fixed array indexes) and string or scalar attributes.

use std::collections::HashSet;

use corvene_models::TensorFile;
use corvene_models::tensors::{
    DType, MetaSection, TensorChunk, TensorFormat, TensorInfo, TensorLocation,
};

use crate::source::{Reader, SubSource, bad};
use crate::zipfile;
use crate::{Error, Source};

const SIGNATURE: &[u8] = b"\x89HDF\r\n\x1a\n";
const MAX_DEPTH: u32 = 64;
const MAX_OBJECTS: usize = 200_000;
const MAX_MESSAGES: usize = 100_000;
const MAX_CHUNKS: usize = 4_000_000;
const MAX_ATTRS: usize = 2000;
const SHOWN_ITEMS: u64 = 5;

struct Msg {
    kind: u16,
    pos: u64,
}

#[derive(Clone)]
struct Dt {
    class: u8,
    size: u64,
    dtype: DType,
    be: bool,
    vlen_str: bool,
}

struct H5<'a> {
    r: Reader<'a>,
    base: u64,
    so: usize,
    sl: usize,
    visited: HashSet<u64>,
    file: TensorFile,
    attrs: Vec<(String, String)>,
    objects: usize,
}

fn le_uint(b: &[u8]) -> u64 {
    b.iter().rev().fold(0u64, |a, x| (a << 8) | *x as u64)
}

impl<'a> H5<'a> {
    fn new(src: &'a dyn Source) -> Self {
        H5 {
            r: Reader::new(src),
            base: 0,
            so: 8,
            sl: 8,
            visited: HashSet::new(),
            file: TensorFile::default(),
            attrs: Vec::new(),
            objects: 0,
        }
    }

    /// An address field; `None` for the undefined address.
    fn addr(&mut self) -> Result<Option<u64>, Error> {
        let v = self.r.uint(self.so)?;
        let undefined = if self.so == 8 {
            u64::MAX
        } else {
            (1u64 << (8 * self.so)) - 1
        };
        Ok((v != undefined).then(|| v.saturating_add(self.base)))
    }
    fn length(&mut self) -> Result<u64, Error> {
        self.r.uint(self.sl)
    }
    fn sig(&mut self, pos: u64, sig: &[u8]) -> Result<bool, Error> {
        Ok(self.r.bytes_at(pos, sig.len() as u64)? == sig)
    }

    fn superblock(&mut self) -> Result<u64, Error> {
        let mut pos = 0u64;
        let len = self.r.len();
        let sb = loop {
            if pos + 8 > len {
                return Err(bad("not an HDF5 file"));
            }
            if self.sig(pos, SIGNATURE)? {
                break pos;
            }
            pos = if pos == 0 { 512 } else { pos * 2 };
        };
        self.r.seek(sb + 8);
        let version = self.r.u8()?;
        self.file
            .metadata
            .push(("Superblock version".into(), version.to_string()));
        match version {
            0 | 1 => {
                self.r.skip(4)?;
                self.so = self.r.u8()? as usize;
                self.sl = self.r.u8()? as usize;
                self.check_sizes()?;
                self.r.skip(1 + 4 + 4)?;
                if version == 1 {
                    self.r.skip(4)?;
                }
                let base = self.r.uint(self.so)?;
                self.base = if base == 0 { sb } else { base };
                for _ in 0..3 {
                    self.r.uint(self.so)?;
                }
                // Root symbol table entry: name offset, header address.
                self.r.uint(self.so)?;
                self.addr()?.ok_or_else(|| bad("HDF5 root group missing"))
            }
            2 | 3 => {
                self.so = self.r.u8()? as usize;
                self.sl = self.r.u8()? as usize;
                self.check_sizes()?;
                self.r.skip(1)?;
                let base = self.r.uint(self.so)?;
                self.base = if base == 0 { sb } else { base };
                self.r.uint(self.so)?;
                self.r.uint(self.so)?;
                self.addr()?.ok_or_else(|| bad("HDF5 root group missing"))
            }
            v => Err(bad(format!("HDF5 superblock version {v} not supported"))),
        }
    }

    fn check_sizes(&self) -> Result<(), Error> {
        if ![2, 4, 8].contains(&self.so) || ![2, 4, 8].contains(&self.sl) {
            return Err(bad("HDF5 address sizes not supported"));
        }
        Ok(())
    }

    /// Every header message of the object at `addr`, following
    /// continuation blocks.
    fn messages(&mut self, addr: u64) -> Result<Vec<Msg>, Error> {
        let mut out = Vec::new();
        let v2 = self.sig(addr, b"OHDR")?;
        // (start, end, is_v2_block)
        let mut blocks: Vec<(u64, u64)> = Vec::new();
        let mut seen = HashSet::new();
        let mut track_crt = false;
        if v2 {
            self.r.seek(addr + 4);
            let version = self.r.u8()?;
            if version != 2 {
                return Err(bad("HDF5 object header version not supported"));
            }
            let flags = self.r.u8()?;
            if flags & 0x20 != 0 {
                self.r.skip(16)?;
            }
            if flags & 0x10 != 0 {
                self.r.skip(4)?;
            }
            track_crt = flags & 0x04 != 0;
            let size = self.r.uint(1 << (flags & 3))?;
            let start = self.r.pos();
            blocks.push((start, start.saturating_add(size)));
        } else {
            self.r.seek(addr);
            let version = self.r.u8()?;
            if version != 1 {
                return Err(bad("HDF5 object header version not supported"));
            }
            self.r.skip(1)?;
            let _n = self.r.u16()?;
            self.r.skip(4)?;
            let size = self.r.u32()? as u64;
            let start = addr + 16;
            blocks.push((start, start + size));
        }
        let mut i = 0;
        while i < blocks.len() {
            let (start, end) = blocks[i];
            i += 1;
            if !seen.insert(start) || blocks.len() > 1000 || end > self.r.len() {
                continue;
            }
            let mut pos = start;
            let hdr = if v2 { if track_crt { 6 } else { 4 } } else { 8 };
            while pos + hdr <= end && out.len() < MAX_MESSAGES {
                self.r.seek(pos);
                let (kind, size) = if v2 {
                    let k = self.r.u8()? as u16;
                    let s = self.r.u16()? as u64;
                    self.r.skip(hdr - 3)?;
                    (k, s)
                } else {
                    let k = self.r.u16()?;
                    let s = self.r.u16()? as u64;
                    self.r.skip(4)?;
                    (k, s)
                };
                let data = pos + hdr;
                if data + size > end {
                    break;
                }
                if kind == 0x10 {
                    self.r.seek(data);
                    if let Some(a) = self.addr()? {
                        let len = self.length()?;
                        if v2 {
                            if self.sig(a, b"OCHK")? && len >= 8 {
                                blocks.push((a + 4, a + len - 4));
                            }
                        } else {
                            blocks.push((a, a.saturating_add(len)));
                        }
                    }
                } else {
                    out.push(Msg { kind, pos: data });
                }
                pos = data + size;
            }
        }
        Ok(out)
    }

    fn datatype(&mut self, pos: u64) -> Result<Dt, Error> {
        self.r.seek(pos);
        let cv = self.r.u8()?;
        let bits = self.r.take(3)?.to_vec();
        let size = self.r.u32()? as u64;
        let class = cv & 0x0f;
        let be = bits[0] & 1 != 0;
        let mut dt = Dt {
            class,
            size,
            dtype: DType::Other(format!("class {class}")),
            be,
            vlen_str: false,
        };
        dt.dtype = match class {
            0 => {
                let signed = bits[0] & 0x08 != 0;
                match (size, signed) {
                    (1, true) => DType::I8,
                    (1, false) => DType::U8,
                    (2, true) => DType::I16,
                    (2, false) => DType::U16,
                    (4, true) => DType::I32,
                    (4, false) => DType::U32,
                    (8, true) => DType::I64,
                    (8, false) => DType::U64,
                    _ => DType::Other(format!("int{}", size * 8)),
                }
            }
            1 => {
                self.r.skip(4)?;
                let _exp_loc = self.r.u8()?;
                let exp_size = self.r.u8()?;
                match size {
                    2 if exp_size == 8 => DType::BF16,
                    2 => DType::F16,
                    4 => DType::F32,
                    8 => DType::F64,
                    _ => DType::Other(format!("float{}", size * 8)),
                }
            }
            3 => DType::Str,
            6 => DType::Other("compound".into()),
            7 => DType::Other("reference".into()),
            8 => {
                // h5py stores bools as an enum over int8.
                let base = self.datatype(pos + 8)?;
                if size == 1 { DType::Bool } else { base.dtype }
            }
            9 => {
                dt.vlen_str = bits[0] & 0x0f == 1;
                if dt.vlen_str {
                    DType::Str
                } else {
                    DType::Other("vlen".into())
                }
            }
            10 => DType::Other("array".into()),
            _ => DType::Other(format!("class {class}")),
        };
        if class == 8 && dt.dtype == DType::Bool {
            dt.be = false;
        }
        Ok(dt)
    }

    /// Shape of a dataspace message; `None` for a null dataspace.
    fn dataspace(&mut self, pos: u64) -> Result<Option<Vec<u64>>, Error> {
        self.r.seek(pos);
        let version = self.r.u8()?;
        let rank = self.r.u8()? as usize;
        let _flags = self.r.u8()?;
        if rank > 32 {
            return Err(bad("HDF5 dataspace rank too large"));
        }
        match version {
            1 => self.r.skip(5)?,
            2 => {
                let ty = self.r.u8()?;
                if ty == 2 {
                    return Ok(None);
                }
            }
            v => return Err(bad(format!("HDF5 dataspace version {v}"))),
        }
        let mut dims = Vec::with_capacity(rank);
        for _ in 0..rank {
            dims.push(self.length()?);
        }
        Ok(Some(dims))
    }

    fn filters(&mut self, pos: u64) -> Result<Vec<u32>, Error> {
        self.r.seek(pos);
        let version = self.r.u8()?;
        let n = self.r.u8()?;
        if version == 1 {
            self.r.skip(6)?;
        }
        let mut ids = Vec::new();
        for _ in 0..n {
            let id = self.r.u16()?;
            let name_len = if version == 1 || id >= 256 {
                self.r.u16()? as u64
            } else {
                0
            };
            let _flags = self.r.u16()?;
            let nv = self.r.u16()? as u64;
            if version == 1 {
                self.r.skip(name_len.div_ceil(8) * 8)?;
                self.r.skip(nv * 4 + if nv % 2 == 1 { 4 } else { 0 })?;
            } else {
                self.r.skip(name_len)?;
                self.r.skip(nv * 4)?;
            }
            ids.push(id as u32);
        }
        Ok(ids)
    }

    /// Reads the bytes of a variable-length item from the global heap.
    fn global_heap(&mut self, coll: u64, index: u32) -> Result<Vec<u8>, Error> {
        if !self.sig(coll, b"GCOL")? {
            return Err(bad("HDF5 global heap missing"));
        }
        self.r.seek(coll + 8);
        let size = self.length()?;
        let end = coll.saturating_add(size).min(self.r.len());
        let mut pos = self.r.pos();
        while pos + 8 + self.sl as u64 <= end {
            self.r.seek(pos);
            let idx = self.r.u16()?;
            self.r.skip(6)?;
            let len = self.length()?;
            let data = self.r.pos();
            if idx == 0 {
                break;
            }
            if idx as u32 == index {
                return self.r.bytes_at(data, len.min(1 << 20));
            }
            pos = data.saturating_add(len.div_ceil(8) * 8);
        }
        Err(bad("HDF5 global heap object missing"))
    }

    /// Up to `SHOWN_ITEMS` items of an attribute's value, as text.
    fn attr_value(&mut self, dt: &Dt, shape: &[u64], data: u64) -> Result<String, Error> {
        let n = crate::elements(shape).unwrap_or(0);
        let show = n.min(SHOWN_ITEMS);
        let mut items = Vec::new();
        for i in 0..show {
            let at = data + i * dt.size;
            let item = if dt.vlen_str {
                self.r.seek(at);
                let _len = self.r.u32()?;
                let coll = self.addr()?;
                let idx = self.r.u32()?;
                match coll {
                    Some(c) => String::from_utf8_lossy(&self.global_heap(c, idx)?).into_owned(),
                    None => String::new(),
                }
            } else if dt.class == 3 {
                let b = self.r.bytes_at(at, dt.size.min(4096))?;
                String::from_utf8_lossy(&b)
                    .trim_end_matches(['\0', ' '])
                    .to_string()
            } else if let Some(es) = dt.dtype.size() {
                let mut b = self.r.bytes_at(at, es)?;
                if dt.be {
                    b.reverse();
                }
                scalar_text(&dt.dtype, &b)
            } else {
                return Ok(format!("{} {}", dt.dtype, shape_text(shape)));
            };
            items.push(crate::clip(&item, 200));
        }
        Ok(if shape.is_empty() {
            items.into_iter().next().unwrap_or_default()
        } else {
            crate::summarize(&items, n)
        })
    }

    /// Parses an attribute message; returns its name, value and length.
    fn attribute(&mut self, pos: u64) -> Result<(String, String, u64), Error> {
        self.r.seek(pos);
        let version = self.r.u8()?;
        self.r.skip(1)?;
        let name_len = self.r.u16()? as u64;
        let dt_len = self.r.u16()? as u64;
        let ds_len = self.r.u16()? as u64;
        let pad = |n: u64| if version == 1 { n.div_ceil(8) * 8 } else { n };
        let mut p = pos + 8;
        if version == 3 {
            p = pos + 9;
        } else if version != 1 && version != 2 {
            return Err(bad("HDF5 attribute version not supported"));
        }
        let name_b = self.r.bytes_at(p, name_len)?;
        let name = String::from_utf8_lossy(&name_b)
            .trim_end_matches('\0')
            .to_string();
        p += pad(name_len);
        let dt = self.datatype(p)?;
        p += pad(dt_len);
        let shape = self.dataspace(p)?.unwrap_or_default();
        p += pad(ds_len);
        let n = crate::elements(&shape).unwrap_or(0);
        let data_len = n.saturating_mul(dt.size);
        let value = self.attr_value(&dt, &shape, p)?;
        Ok((name, value, (p - pos).saturating_add(data_len)))
    }

    /// The managed-object regions of a fractal heap's direct blocks, in
    /// heap order, and the heap's managed object count.
    fn heap_blocks(&mut self, heap: u64) -> Result<(Vec<(u64, u64)>, u64), Error> {
        if !self.sig(heap, b"FRHP")? {
            return Err(bad("HDF5 fractal heap missing"));
        }
        self.r.seek(heap + 5);
        let _id_len = self.r.u16()?;
        let io_len = self.r.u16()?;
        let flags = self.r.u8()?;
        if io_len > 0 {
            return Err(bad("filtered fractal heaps not supported"));
        }
        self.r.skip(4)?;
        self.length()?;
        self.addr()?;
        self.length()?;
        self.addr()?;
        for _ in 0..3 {
            self.length()?;
        }
        let objects = self.length()?;
        for _ in 0..4 {
            self.length()?;
        }
        let width = self.r.u16()? as u64;
        let start_size = self.length()?;
        let max_direct = self.length()?;
        let max_heap_bits = self.r.u16()? as u64;
        let _start_rows = self.r.u16()?;
        let root = self.addr()?;
        let rows = self.r.u16()? as u64;
        if width == 0
            || width > 1 << 16
            || !start_size.is_power_of_two()
            || !max_direct.is_power_of_two()
        {
            return Err(bad("HDF5 fractal heap layout not supported"));
        }
        let off_bytes = max_heap_bits.div_ceil(8);
        let checksum = flags & 2 != 0;
        let mut blocks = Vec::new();
        let Some(root) = root else {
            return Ok((blocks, objects));
        };
        let max_direct_rows = (max_direct.ilog2() - start_size.ilog2()) as u64 + 2;
        let geo = HeapGeo {
            width,
            start_size,
            max_direct_rows,
            off_bytes,
            checksum,
        };
        if rows == 0 {
            self.direct_block(root, start_size, &geo, &mut blocks)?;
        } else {
            self.indirect_block(root, rows, &geo, &mut blocks, 0)?;
        }
        Ok((blocks, objects))
    }

    fn direct_block(
        &mut self,
        addr: u64,
        size: u64,
        geo: &HeapGeo,
        out: &mut Vec<(u64, u64)>,
    ) -> Result<(), Error> {
        if !self.sig(addr, b"FHDB")? {
            return Err(bad("HDF5 heap block missing"));
        }
        let header = 5 + self.so as u64 + geo.off_bytes + if geo.checksum { 4 } else { 0 };
        if out.len() < 100_000 {
            out.push((addr + header, addr + size));
        }
        Ok(())
    }

    fn indirect_block(
        &mut self,
        addr: u64,
        rows: u64,
        geo: &HeapGeo,
        out: &mut Vec<(u64, u64)>,
        depth: u32,
    ) -> Result<(), Error> {
        if depth > 8 || !self.sig(addr, b"FHIB")? || rows > 64 {
            return Err(bad("HDF5 heap block missing"));
        }
        let mut pos = addr + 5 + self.so as u64 + geo.off_bytes;
        let mut children = Vec::new();
        for r in 0..rows {
            for _ in 0..geo.width {
                self.r.seek(pos);
                let a = self.addr()?;
                pos += self.so as u64;
                let size = if r < 2 {
                    geo.start_size
                } else {
                    geo.start_size.saturating_mul(1 << (r - 1).min(62))
                };
                children.push((a, r, size));
            }
        }
        for (a, r, size) in children {
            let Some(a) = a else { continue };
            if r < geo.max_direct_rows {
                self.direct_block(a, size, geo, out)?;
            } else {
                let sub_rows = (size.ilog2() as u64)
                    .saturating_sub((geo.start_size * geo.width).ilog2() as u64)
                    + 1;
                self.indirect_block(a, sub_rows, geo, out, depth + 1)?;
            }
        }
        Ok(())
    }

    /// Parses `parse` objects back to back in a heap's blocks.
    fn heap_scan<T>(
        &mut self,
        heap: u64,
        mut parse: impl FnMut(&mut Self, u64) -> Result<(T, u64), Error>,
    ) -> Result<(Vec<T>, bool), Error> {
        let (blocks, objects) = self.heap_blocks(heap)?;
        let mut out = Vec::new();
        for (start, end) in blocks {
            let mut pos = start;
            while pos < end && (out.len() as u64) < objects {
                if self.r.bytes_at(pos, 1)?.first() == Some(&0) {
                    break;
                }
                match parse(self, pos) {
                    Ok((v, len)) if len > 0 && pos + len <= end => {
                        out.push(v);
                        pos += len;
                    }
                    _ => break,
                }
            }
        }
        let complete = out.len() as u64 == objects;
        Ok((out, complete))
    }

    /// A link message: name, target address (hard links) and length.
    fn link(&mut self, pos: u64) -> Result<((String, Option<u64>), u64), Error> {
        self.r.seek(pos);
        if self.r.u8()? != 1 {
            return Err(bad("HDF5 link version"));
        }
        let flags = self.r.u8()?;
        let ty = if flags & 0x08 != 0 { self.r.u8()? } else { 0 };
        if flags & 0x04 != 0 {
            self.r.skip(8)?;
        }
        if flags & 0x10 != 0 {
            self.r.skip(1)?;
        }
        let len = self.r.uint(1 << (flags & 3))?;
        let name = self.r.string(len.min(65536))?;
        let target = match ty {
            0 => self.addr()?,
            _ => {
                let n = self.r.u16()? as u64;
                self.r.skip(n)?;
                None
            }
        };
        Ok(((name, target), self.r.pos() - pos))
    }

    fn local_heap(&mut self, addr: u64) -> Result<(u64, u64), Error> {
        if !self.sig(addr, b"HEAP")? {
            return Err(bad("HDF5 local heap missing"));
        }
        self.r.seek(addr + 8);
        let size = self.length()?;
        self.length()?;
        let data = self.addr()?.ok_or_else(|| bad("HDF5 local heap empty"))?;
        Ok((data, size))
    }

    fn heap_name(&mut self, heap: (u64, u64), off: u64) -> Result<String, Error> {
        if off >= heap.1 {
            return Err(bad("HDF5 name out of its heap"));
        }
        let n = (heap.1 - off).min(4096);
        let b = self.r.bytes_at(heap.0 + off, n)?;
        let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }

    /// Walks a v1 B-tree; `leaf` gets each (key position, child address).
    fn btree_v1(
        &mut self,
        addr: u64,
        key_size: u64,
        depth: u32,
        seen: &mut HashSet<u64>,
        leaf: &mut dyn FnMut(&mut Self, u64, u64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if depth > 32 || !seen.insert(addr) || !self.sig(addr, b"TREE")? {
            return Err(bad("HDF5 B-tree node missing"));
        }
        self.r.seek(addr + 5);
        let level = self.r.u8()?;
        let entries = self.r.u16()? as u64;
        let mut pos = addr + 8 + 2 * self.so as u64;
        let mut kids = Vec::with_capacity(entries as usize);
        for _ in 0..entries {
            let key = pos;
            self.r.seek(pos + key_size);
            let child = self.addr()?;
            pos += key_size + self.so as u64;
            kids.push((key, child));
        }
        for (key, child) in kids {
            let Some(child) = child else { continue };
            if level > 0 {
                self.btree_v1(child, key_size, depth + 1, seen, leaf)?;
            } else {
                leaf(self, key, child)?;
            }
        }
        Ok(())
    }

    fn symbol_table(&mut self, msg: &Msg) -> Result<Vec<(String, Option<u64>)>, Error> {
        self.r.seek(msg.pos);
        let tree = self.addr()?;
        let heap = self.addr()?;
        let (Some(tree), Some(heap)) = (tree, heap) else {
            return Ok(Vec::new());
        };
        let heap = self.local_heap(heap)?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let key = self.sl as u64;
        let entry = 2 * self.so as u64 + 24;
        self.btree_v1(tree, key, 0, &mut seen, &mut |h, _key, snod| {
            if !h.sig(snod, b"SNOD")? {
                return Err(bad("HDF5 symbol node missing"));
            }
            h.r.seek(snod + 6);
            let n = h.r.u16()? as u64;
            for i in 0..n {
                h.r.seek(snod + 8 + i * entry);
                let name_off = h.r.uint(h.so)?;
                let obj = h.addr()?;
                let name = h.heap_name(heap, name_off)?;
                if out.len() < MAX_OBJECTS {
                    out.push((name, obj));
                }
            }
            Ok(())
        })?;
        Ok(out)
    }

    fn walk(&mut self, addr: u64, path: &str, depth: u32) {
        if let Err(e) = self.object(addr, path, depth) {
            self.file.notes.push(format!("{path}: {e}"));
        }
    }

    fn object(&mut self, addr: u64, path: &str, depth: u32) -> Result<(), Error> {
        self.objects += 1;
        if depth > MAX_DEPTH || self.objects > MAX_OBJECTS || !self.visited.insert(addr) {
            return Ok(());
        }
        let msgs = self.messages(addr)?;
        let mut children: Vec<(String, Option<u64>)> = Vec::new();
        let (mut dt, mut shape, mut layout, mut filters) = (None, None, None, Vec::new());
        for m in &msgs {
            match m.kind {
                0x01 => shape = Some(self.dataspace(m.pos)?),
                0x03 => dt = Some(self.datatype(m.pos)?),
                0x08 => layout = Some(m.pos),
                0x0B => filters = self.filters(m.pos)?,
                0x0C => {
                    let (name, value, _) = self.attribute(m.pos)?;
                    self.push_attr(path, name, value);
                }
                0x15 => {
                    self.r.seek(m.pos);
                    let _v = self.r.u8()?;
                    let flags = self.r.u8()?;
                    if flags & 1 != 0 {
                        self.r.skip(2)?;
                    }
                    if let Some(heap) = self.addr()? {
                        let (attrs, complete) = self.heap_scan(heap, |h, p| {
                            let (n, v, len) = h.attribute(p)?;
                            Ok(((n, v), len))
                        })?;
                        for (n, v) in attrs {
                            self.push_attr(path, n, v);
                        }
                        if !complete {
                            self.file
                                .notes
                                .push(format!("{path}: some attributes not read"));
                        }
                    }
                }
                0x11 => children.extend(self.symbol_table(m)?),
                0x06 => children.push(self.link(m.pos)?.0),
                0x02 => {
                    self.r.seek(m.pos);
                    let _v = self.r.u8()?;
                    let flags = self.r.u8()?;
                    if flags & 1 != 0 {
                        self.r.skip(8)?;
                    }
                    if let Some(heap) = self.addr()? {
                        let (links, complete) = self.heap_scan(heap, |h, p| h.link(p))?;
                        children.extend(links);
                        if !complete {
                            self.file
                                .notes
                                .push(format!("{path}: group read partially (dense link storage)"));
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(layout) = layout {
            let dt = dt.ok_or_else(|| bad("dataset without a datatype"))?;
            let shape = shape.ok_or_else(|| bad("dataset without a dataspace"))?;
            let info = self.dataset(path, &dt, shape, layout, filters)?;
            self.file.tensors.push(info);
        }
        for (name, target) in children {
            let child = if path == "/" {
                format!("/{name}")
            } else {
                format!("{path}/{name}")
            };
            match target {
                Some(a) => self.walk(a, &child, depth + 1),
                None => self
                    .file
                    .notes
                    .push(format!("{child}: soft or external link not followed")),
            }
        }
        Ok(())
    }

    fn push_attr(&mut self, path: &str, name: String, value: String) {
        if path == "/" {
            self.file.metadata.push((name, value));
        } else if self.attrs.len() < MAX_ATTRS {
            self.attrs.push((format!("{path} @{name}"), value));
        }
    }

    fn dataset(
        &mut self,
        path: &str,
        dt: &Dt,
        shape: Option<Vec<u64>>,
        pos: u64,
        filters: Vec<u32>,
    ) -> Result<TensorInfo, Error> {
        let shape = shape.unwrap_or_default();
        let elements = crate::elements(&shape).ok_or_else(|| bad("shape overflows"))?;
        let mut info = TensorInfo {
            name: path.to_string(),
            dtype: dt.dtype.clone(),
            shape: shape.clone(),
            elements,
            big_endian: dt.be && dt.size > 1,
            ..TensorInfo::default()
        };
        let dense = elements.saturating_mul(dt.size);
        let len = self.r.len();
        self.r.seek(pos);
        let version = self.r.u8()?;
        let rank = shape.len();
        let (class, chunk_dims, chunk_index): (u8, Vec<u64>, ChunkIndex) = match version {
            1 | 2 => {
                let dims = self.r.u8()? as usize;
                let class = self.r.u8()?;
                self.r.skip(5)?;
                let addr = if class != 0 { self.addr()? } else { None };
                let mut d = Vec::new();
                for _ in 0..dims.min(33) {
                    d.push(self.r.u32()? as u64);
                }
                match class {
                    0 => {
                        let size = self.r.u32()? as u64;
                        let at = self.r.pos();
                        info.bytes = size;
                        info.location = TensorLocation::Bytes {
                            offset: at,
                            len: size.min(dense),
                        };
                        return Ok(info);
                    }
                    1 => {
                        info.bytes = dense;
                        if let Some(a) = addr.filter(|a| a.saturating_add(dense) <= len) {
                            info.location = TensorLocation::Bytes {
                                offset: a,
                                len: dense,
                            };
                        }
                        return Ok(info);
                    }
                    _ => (2, d, ChunkIndex::BTree(addr)),
                }
            }
            3 | 4 => {
                let class = self.r.u8()?;
                match class {
                    0 => {
                        let size = self.r.u16()? as u64;
                        let at = self.r.pos();
                        info.bytes = size;
                        info.location = TensorLocation::Bytes {
                            offset: at,
                            len: size.min(dense),
                        };
                        return Ok(info);
                    }
                    1 => {
                        let addr = self.addr()?;
                        let size = self.length()?;
                        info.bytes = size;
                        if let Some(a) =
                            addr.filter(|a| a.saturating_add(size) <= len && size == dense)
                        {
                            info.location = TensorLocation::Bytes {
                                offset: a,
                                len: size,
                            };
                        }
                        return Ok(info);
                    }
                    2 if version == 3 => {
                        let dims = self.r.u8()? as usize;
                        let addr = self.addr()?;
                        let mut d = Vec::new();
                        for _ in 0..dims.min(33) {
                            d.push(self.r.u32()? as u64);
                        }
                        (2, d, ChunkIndex::BTree(addr))
                    }
                    2 => {
                        let flags = self.r.u8()?;
                        let dims = self.r.u8()? as usize;
                        let enc = self.r.u8()? as usize;
                        let mut d = Vec::new();
                        for _ in 0..dims.min(33) {
                            d.push(self.r.uint(enc)?);
                        }
                        let ty = self.r.u8()?;
                        let idx = match ty {
                            1 => {
                                let (size, mask) = if flags & 2 != 0 {
                                    (Some(self.length()?), self.r.u32()?)
                                } else {
                                    (None, 0)
                                };
                                ChunkIndex::Single(self.addr()?, size, mask)
                            }
                            2 => ChunkIndex::Implicit(self.addr()?),
                            3 => {
                                self.r.skip(1)?;
                                ChunkIndex::Fixed(self.addr()?)
                            }
                            t => ChunkIndex::Unsupported(t),
                        };
                        (2, d, idx)
                    }
                    c => {
                        self.file
                            .notes
                            .push(format!("{path}: layout class {c} not read"));
                        return Ok(info);
                    }
                }
            }
            v => return Err(bad(format!("HDF5 layout version {v}"))),
        };
        let _ = class;
        // The last chunk dimension is the element size.
        if chunk_dims.len() != rank + 1 || rank == 0 {
            return Err(bad("chunk dimensions do not match the dataspace"));
        }
        let cshape: Vec<u64> = chunk_dims[..rank].to_vec();
        if cshape.contains(&0) {
            return Err(bad("empty chunk dimension"));
        }
        let chunk_bytes = crate::elements(&cshape)
            .ok_or_else(|| bad("chunk overflows"))?
            .saturating_mul(dt.size);
        let grid: Vec<u64> = shape
            .iter()
            .zip(&cshape)
            .map(|(s, c)| s.div_ceil(*c))
            .collect();
        let nchunks = crate::elements(&grid).unwrap_or(u64::MAX);
        let coords_of = |i: u64| -> Vec<u64> {
            let mut c = vec![0u64; rank];
            let mut rest = i;
            for k in (0..rank).rev() {
                c[k] = (rest % grid[k].max(1)) * cshape[k];
                rest /= grid[k].max(1);
            }
            c
        };
        let mut chunks = Vec::new();
        match chunk_index {
            ChunkIndex::BTree(Some(tree)) => {
                let key = 8 + 8 * (rank as u64 + 1);
                let mut seen = HashSet::new();
                self.btree_v1(tree, key, 0, &mut seen, &mut |h, key, child| {
                    h.r.seek(key);
                    let size = h.r.u32()? as u64;
                    let mask = h.r.u32()?;
                    let mut coords = Vec::with_capacity(rank);
                    for _ in 0..rank {
                        coords.push(h.r.u64()?);
                    }
                    if chunks.len() < MAX_CHUNKS {
                        chunks.push(TensorChunk {
                            offset: child,
                            size,
                            filter_mask: mask,
                            coords,
                        });
                    }
                    Ok(())
                })?;
            }
            ChunkIndex::Single(Some(a), size, mask) => chunks.push(TensorChunk {
                offset: a,
                size: size.unwrap_or(chunk_bytes),
                filter_mask: mask,
                coords: vec![0; rank],
            }),
            ChunkIndex::Implicit(Some(a)) => {
                if nchunks > MAX_CHUNKS as u64 {
                    return Err(bad("too many chunks"));
                }
                for i in 0..nchunks {
                    chunks.push(TensorChunk {
                        offset: a + i * chunk_bytes,
                        size: chunk_bytes,
                        filter_mask: 0,
                        coords: coords_of(i),
                    });
                }
            }
            ChunkIndex::Fixed(Some(hdr)) => {
                if !self.sig(hdr, b"FAHD")? {
                    return Err(bad("HDF5 fixed array missing"));
                }
                self.r.seek(hdr + 5);
                let client = self.r.u8()?;
                let entry = self.r.u8()? as u64;
                let page_bits = self.r.u8()?;
                let n = self.length()?;
                let block = self.addr()?;
                if n > (1u64 << page_bits.min(63)) {
                    self.file
                        .notes
                        .push(format!("{path}: paged chunk index not read"));
                } else if let Some(block) = block {
                    if n > MAX_CHUNKS as u64 || !self.sig(block, b"FADB")? {
                        return Err(bad("HDF5 fixed array block missing"));
                    }
                    let mut p = block + 6 + self.so as u64;
                    for i in 0..n {
                        self.r.seek(p);
                        let a = self.addr()?;
                        let (size, mask) = if client == 1 {
                            let w = entry.saturating_sub(self.so as u64 + 4) as usize;
                            (self.r.uint(w)?, self.r.u32()?)
                        } else {
                            (chunk_bytes, 0)
                        };
                        p += entry;
                        if let Some(a) = a {
                            chunks.push(TensorChunk {
                                offset: a,
                                size,
                                filter_mask: mask,
                                coords: coords_of(i),
                            });
                        }
                    }
                }
            }
            ChunkIndex::Unsupported(t) => {
                self.file
                    .notes
                    .push(format!("{path}: chunk index type {t} not read"));
            }
            _ => {}
        }
        info.bytes = chunks.iter().fold(0u64, |a, c| a.saturating_add(c.size));
        chunks.retain(|c| c.offset.saturating_add(c.size) <= len);
        if !chunks.is_empty() || info.bytes == 0 {
            info.location = TensorLocation::Chunked {
                chunk_shape: cshape,
                filters,
                chunks,
            };
        }
        Ok(info)
    }
}

struct HeapGeo {
    width: u64,
    start_size: u64,
    max_direct_rows: u64,
    off_bytes: u64,
    checksum: bool,
}

enum ChunkIndex {
    BTree(Option<u64>),
    Single(Option<u64>, Option<u64>, u32),
    Implicit(Option<u64>),
    Fixed(Option<u64>),
    Unsupported(u8),
}

fn scalar_text(dtype: &DType, b: &[u8]) -> String {
    let u = le_uint(b);
    match dtype {
        DType::Bool => (u != 0).to_string(),
        DType::I8 => (u as u8 as i8).to_string(),
        DType::I16 => (u as u16 as i16).to_string(),
        DType::I32 => (u as u32 as i32).to_string(),
        DType::I64 => (u as i64).to_string(),
        DType::F16 => crate::fmt_f64(half::f16::from_bits(u as u16).to_f64()),
        DType::BF16 => crate::fmt_f64(half::bf16::from_bits(u as u16).to_f64()),
        DType::F32 => crate::fmt_f64(f32::from_bits(u as u32) as f64),
        DType::F64 => crate::fmt_f64(f64::from_bits(u)),
        _ => u.to_string(),
    }
}

fn shape_text(shape: &[u64]) -> String {
    let d: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
    format!("[{}]", d.join(", "))
}

pub(crate) fn read(src: &dyn Source, format: TensorFormat) -> Result<TensorFile, Error> {
    let mut h = H5::new(src);
    let root = h.superblock()?;
    h.file.format = format;
    h.walk(root, "/", 0);
    let mut file = std::mem::take(&mut h.file);
    if !h.attrs.is_empty() {
        file.sections.push(MetaSection {
            title: "Attributes".into(),
            entries: std::mem::take(&mut h.attrs),
        });
    }
    Ok(file)
}

/// Shifts every location of `file` by `by` bytes (an HDF5 file stored
/// inside a zip).
fn shift(file: &mut TensorFile, by: u64) {
    for t in &mut file.tensors {
        match &mut t.location {
            TensorLocation::Bytes { offset, .. } | TensorLocation::Strided { offset, .. } => {
                *offset += by
            }
            TensorLocation::Varints { offset, .. } => *offset += by,
            TensorLocation::Chunked { chunks, .. } => {
                chunks.iter_mut().for_each(|c| c.offset += by)
            }
            TensorLocation::Missing | TensorLocation::External { .. } => {}
        }
    }
}

/// A Keras 3 `.keras` zip: `metadata.json`, `config.json` and
/// `model.weights.h5`.
pub(crate) fn read_keras(src: &dyn Source) -> Result<TensorFile, Error> {
    if !zipfile::is_zip(src) {
        // Keras 2 could save `.keras` as plain HDF5 too.
        return read(src, TensorFormat::Keras);
    }
    let members = zipfile::list(src)?;
    let mut meta = Vec::new();
    if let Some(m) = members.iter().find(|m| m.name == "metadata.json")
        && let Ok(b) = zipfile::read_member(src, m, 1 << 20)
        && let Ok(serde_json::Value::Object(o)) = serde_json::from_slice::<serde_json::Value>(&b)
    {
        for (k, v) in o {
            let s = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            meta.push((k, crate::clip(&s, 500)));
        }
    }
    if let Some(m) = members.iter().find(|m| m.name == "config.json")
        && let Ok(b) = zipfile::read_member(src, m, 64 << 20)
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b)
    {
        if let Some(c) = v.get("class_name").and_then(|c| c.as_str()) {
            meta.push(("Model class".into(), c.to_string()));
        }
        if let Some(n) = v.pointer("/config/name").and_then(|c| c.as_str()) {
            meta.push(("Model name".into(), n.to_string()));
        }
        if let Some(l) = v.pointer("/config/layers").and_then(|c| c.as_array()) {
            meta.push(("Layers".into(), l.len().to_string()));
        }
    }
    let weights = members
        .iter()
        .find(|m| m.name == "model.weights.h5")
        .ok_or_else(|| bad(".keras archive without model.weights.h5"))?;
    let mut file = if weights.stored {
        let sub = SubSource {
            inner: src,
            offset: weights.data_start,
            len: weights.size,
        };
        let mut f = read(&sub, TensorFormat::Keras)?;
        shift(&mut f, weights.data_start);
        f
    } else {
        let data = zipfile::read_member(src, weights, 256 << 20)?;
        let mut f = read(&data, TensorFormat::Keras)?;
        for t in &mut f.tensors {
            t.member = Some(weights.name.clone());
        }
        f
    };
    file.metadata.splice(0..0, meta);
    if members
        .iter()
        .any(|m| m.name == "assets" || m.name.starts_with("assets/"))
    {
        file.notes.push("assets not read".into());
    }
    Ok(file)
}
