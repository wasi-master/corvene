#!/usr/bin/env python3
"""Build the font-diff test fixtures for crates/corvene-fonts.

Usage:
    python3 tools/fonts/make_fixtures.py <cosmic-text fonts dir> \
        <ttf-parser test fonts dir> <out dir>

The inputs are the fonts that ship inside the cosmic-text and ttf-parser
crates (Inter, Inter Variable, Noto Sans and Fira Mono under the OFL, the
COLRv1 test font under Apache-2.0). Every fixture comes as a v1/v2 pair
with a small, documented set of differences so the Rust side can assert
exactly what a diff reports:

- sans-v1.ttf / sans-v2.ttf: Inter Regular subset to Basic Latin plus
  e-acute, sharp s, en dash and right quote. v2 moves four points of the
  first contour of `a` up by 20 units, widens `n` by 12, adds `Euro` and
  `Omega`, drops `asciitilde`, lowers the T+o kern by 30 (an exception
  pair subtable ahead of the class kerning), drops ss02, adds ss03
  (`g -> g.ss03`, UI name "Open g"), bumps the version (4.000 -> 4.100),
  changes the vendor id (RSMS -> CRVN) and renames `uni00DF` to
  `germandbls` without touching its outline.
- sans-v1.woff / sans-v2.woff2: the same fonts in WOFF and WOFF2.
- sans-cff-v1.otf / sans-cff-v2.otf: the same fonts with CFF outlines.
- family-v1.ttc / family-v2.ttc: collections. v2 has the v2 sans face,
  Noto Sans with an edited `A` and a new third face (Fira Mono).
- var-v1.ttf / var-v2.ttf: Inter Variable subset to "AHOVaehnost0123 ".
  v2 drops the Thin instance, renames ExtraLight to UltraLight and scales
  the gvar deltas of `o` by 1.2 (the default outline is unchanged).
- colr.ttf: ttf-parser's COLRv1 test font, copied unchanged.
- ufo-v1/, ufo-v2/: UFO 3 glyph sources (`a`, `acutecomb`, `aacute`, plus
  a cubic `o` taken from the CFF outlines).
- sample-v1.glyphs / sample-v2.glyphs: Glyphs 3 sources with two masters;
  sample-g2-v1.glyphs is a Glyphs 2 file for parser coverage.
- family-v1.designspace / family-v2.designspace: designspace 5.0 files.

All output is deterministic: head timestamps are pinned, nothing is
random and every collection is iterated in a fixed order, so running the
script twice produces byte-identical files.
"""

import copy
import io
import logging
import math
import os
import plistlib
import shutil
import sys
from types import SimpleNamespace

from fontTools import agl
from fontTools import subset as ftsubset
from fontTools.fontBuilder import FontBuilder
from fontTools.misc.timeTools import timestampFromString
from fontTools.otlLib import builder as otl
from fontTools.pens.basePen import BasePen
from fontTools.pens.pointPen import SegmentToPointPen
from fontTools.pens.recordingPen import DecomposingRecordingPen, RecordingPointPen
from fontTools.pens.t2CharStringPen import T2CharStringPen
from fontTools.ttLib import TTFont
from fontTools.ttLib.tables import otTables
from fontTools.ttLib.ttCollection import TTCollection
from fontTools.ufoLib.glifLib import writeGlyphToString
from fontTools.varLib import instancer

FIXED_TIME = timestampFromString("Mon Jan  1 00:00:00 2024")

SANS_FEATURES = [
    "kern", "liga", "calt", "ss01", "ss02", "case", "tnum", "frac",
    "ccmp", "mark", "mkmk",
]
SANS_V2_FEATURES = [tag for tag in SANS_FEATURES if tag != "ss02"]
BASIC_LATIN = list(range(0x20, 0x7F))
SANS_V1_UNICODES = BASIC_LATIN + [0xE9, 0xDF, 0x2013, 0x2019]
SANS_V2_UNICODES = [u for u in SANS_V1_UNICODES if u != 0x7E] + [0x20AC, 0x3A9]
PLAIN_FEATURES = ["kern", "liga", "calt", "ccmp", "mark", "mkmk"]
VAR_TEXT = "AHOVaehnost0123 "

# AGLFN maps "Omega" to U+2126 only; give the Greek letter its usual name.
EXTRA_AGL = {0x3A9: "Omega"}

KERN_LEFT, KERN_RIGHT, KERN_DELTA = "T", "o", -30
A_POINTS_MOVED, A_DY = 4, 20
N_ADVANCE_DELTA = 12
GVAR_SCALE = 1.2


# ---------------------------------------------------------------- helpers


def load(data):
    """Open font bytes without touching head.modified on save."""
    return TTFont(io.BytesIO(data), recalcTimestamp=False)


def to_bytes(font):
    buf = io.BytesIO()
    font.save(buf)
    return buf.getvalue()


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    with open(path, mode, **({} if mode == "wb" else {"encoding": "utf-8", "newline": "\n"})) as f:
        f.write(data)


def subset(path, unicodes, features, hinting=True):
    """Subset a font file and return the result as bytes."""
    font = TTFont(path, recalcTimestamp=False)
    opts = ftsubset.Options()
    opts.layout_features = features
    opts.name_IDs = ["*"]
    opts.name_languages = ["*"]
    opts.glyph_names = True
    opts.notdef_outline = True
    opts.retain_gids = False
    opts.hinting = hinting
    opts.legacy_kern = False
    sub = ftsubset.Subsetter(opts)
    sub.populate(unicodes=sorted(unicodes))
    sub.subset(font)
    return to_bytes(font)


def agl_names(data, overrides):
    """Map `uniXXXX[.suffix]` names to AGL names (`a`, `Euro`, `a.ss01`)."""
    font = load(data)
    first_cp = {}
    for cp, name in sorted(font.getBestCmap().items()):
        first_cp.setdefault(name, cp)
    order = font.getGlyphOrder()
    taken = set(order)
    mapping = {}
    for name in order:
        base, dot, suffix = name.partition(".")
        cp = first_cp.get(base)
        if cp is None:
            continue
        new = overrides.get(cp) or EXTRA_AGL.get(cp) or agl.UV2AGL.get(cp)
        if not new:
            continue
        new = new + dot + suffix
        if new != name and new not in taken:
            mapping[name] = new
            taken.add(new)
    return mapping


def rename_glyphs(data, mapping):
    """Reload font bytes with glyphs renamed and every table decompiled."""
    font = load(data)
    order = font.getGlyphOrder()
    font.setGlyphOrder([mapping.get(name, name) for name in order])
    font.ensureDecompiled()
    post = font["post"]
    if post.formatType == 2.0:
        post.extraNames = []
        post.mapping = {}
    return font


def pin_timestamps(font):
    font["head"].created = FIXED_TIME
    font["head"].modified = FIXED_TIME


def set_name(font, name_id, text):
    for record in font["name"].names:
        if record.nameID == name_id:
            record.string = text


def set_version(font, version, vendor):
    """Version 4.000 style naming plus head.fontRevision and achVendID."""
    font["head"].fontRevision = version
    set_name(font, 5, "Version %.3f" % version)
    ps_name = font["name"].getDebugName(6)
    set_name(font, 3, "%.3f;%s;%s" % (version, vendor, ps_name))
    font["OS/2"].achVendID = vendor


def move_points(glyph, glyf, count, dy):
    """Move the first `count` points of the first contour by `dy`."""
    coords = glyph.coordinates
    last = min(count, glyph.endPtsOfContours[0] + 1)
    for i in range(last):
        x, y = coords[i]
        coords[i] = (x, y + dy)
    glyph.recalcBounds(glyf)


def lookup_subtables(lookup):
    """Yield (container index, real subtable), unwrapping extensions."""
    for i, sub in enumerate(lookup.SubTable):
        yield i, getattr(sub, "ExtSubTable", sub)


def feature_lookups(table, tag):
    indices = []
    for record in table.FeatureList.FeatureRecord:
        if record.FeatureTag == tag:
            indices.extend(record.Feature.LookupListIndex)
    return sorted(set(indices))


def pair_value(font, left, right):
    """First kerning value a shaper would apply to `left` + `right`."""
    gpos = font["GPOS"].table
    for index in feature_lookups(gpos, "kern"):
        lookup = gpos.LookupList.Lookup[index]
        for _, sub in lookup_subtables(lookup):
            if not hasattr(sub, "Coverage") or left not in sub.Coverage.glyphs:
                continue
            if sub.Format == 1:
                pair_set = sub.PairSet[sub.Coverage.glyphs.index(left)]
                for record in pair_set.PairValueRecord:
                    if record.SecondGlyph == right:
                        return index, getattr(record.Value1, "XAdvance", 0) or 0
                continue
            class1 = sub.ClassDef1.classDefs.get(left, 0)
            class2 = sub.ClassDef2.classDefs.get(right, 0)
            value = sub.Class1Record[class1].Class2Record[class2].Value1
            return index, getattr(value, "XAdvance", 0) or 0
    return None, 0


def override_kern(font, left, right, delta):
    """Put a format 1 exception pair in front of the existing kerning."""
    index, value = pair_value(font, left, right)
    if index is None:
        raise SystemExit("no kerning lookup covers %s %s" % (left, right))
    record = otl.buildValue({"XAdvance": value + delta})
    pair = otl.buildPairPosGlyphsSubtable(
        {(left, right): (record, None)}, font.getReverseGlyphMap()
    )
    lookup = font["GPOS"].table.LookupList.Lookup[index]
    if lookup.LookupType == 9:
        ext = otTables.ExtensionPos()
        ext.Format = 1
        ext.ExtensionLookupType = 2
        ext.ExtSubTable = pair
        pair = ext
    lookup.SubTable.insert(0, pair)
    lookup.SubTableCount = len(lookup.SubTable)
    return value, value + delta


def add_feature(table, tag, lookup_index, params=None):
    """Add a feature to every language system, keeping FeatureList sorted."""
    feature = otTables.Feature()
    feature.FeatureParams = params
    feature.LookupListIndex = [lookup_index]
    feature.LookupCount = 1
    record = otTables.FeatureRecord()
    record.FeatureTag = tag
    record.Feature = feature
    records = table.FeatureList.FeatureRecord + [record]
    order = sorted(range(len(records)), key=lambda i: records[i].FeatureTag)
    remap = {old: new for new, old in enumerate(order)}
    table.FeatureList.FeatureRecord = [records[i] for i in order]
    table.FeatureList.FeatureCount = len(records)
    added = remap[len(records) - 1]
    for script_record in table.ScriptList.ScriptRecord:
        script = script_record.Script
        systems = [script.DefaultLangSys] if script.DefaultLangSys else []
        systems += [lang.LangSys for lang in script.LangSysRecord]
        for system in systems:
            system.FeatureIndex = sorted([remap[i] for i in system.FeatureIndex] + [added])
            system.FeatureCount = len(system.FeatureIndex)
            if system.ReqFeatureIndex != 0xFFFF:
                system.ReqFeatureIndex = remap[system.ReqFeatureIndex]


def add_glyph(font, name, glyph, metrics, glyph_class):
    order = font.getGlyphOrder() + [name]
    font.setGlyphOrder(order)
    font["glyf"].glyphs[name] = glyph
    font["hmtx"].metrics[name] = metrics
    font["maxp"].numGlyphs = len(order)
    gdef = font["GDEF"].table if "GDEF" in font else None
    if gdef is not None and gdef.GlyphClassDef is not None and glyph_class:
        gdef.GlyphClassDef.classDefs[name] = glyph_class


def add_open_g(font):
    """ss03: g -> g.ss03, a copy of `g` with a 30 unit deeper descender."""
    glyf = font["glyf"]
    glyph = copy.deepcopy(glyf["g"])
    coords = glyph.coordinates
    for i, (x, y) in enumerate(coords):
        if y < 0:
            coords[i] = (x, y - 30)
    glyph.recalcBounds(glyf)
    gdef = font["GDEF"].table if "GDEF" in font else None
    g_class = gdef.GlyphClassDef.classDefs.get("g", 1) if gdef and gdef.GlyphClassDef else 1
    add_glyph(font, "g.ss03", glyph, font["hmtx"].metrics["g"], g_class)

    gsub = font["GSUB"].table
    lookup = otl.buildLookup([otl.buildSingleSubstSubtable({"g": "g.ss03"})])
    gsub.LookupList.Lookup.append(lookup)
    gsub.LookupList.LookupCount = len(gsub.LookupList.Lookup)
    params = otTables.FeatureParamsStylisticSet()
    params.Version = 0
    params.UINameID = font["name"].addName("Open g", platforms=((3, 1, 0x409),))
    add_feature(gsub, "ss03", len(gsub.LookupList.Lookup) - 1, params)
    return params.UINameID


# ---------------------------------------------------------------- sans


def build_sans(src, version):
    """Inter Regular subset; version 2 carries every documented edit."""
    path = os.path.join(src, "Inter-Regular.ttf")
    if version == 1:
        data = subset(path, SANS_V1_UNICODES, SANS_FEATURES)
        font = rename_glyphs(data, agl_names(data, {0xDF: "uni00DF"}))
        set_version(font, 4.0, "RSMS")
        pin_timestamps(font)
        return load(to_bytes(font)), {}

    data = subset(path, SANS_V2_UNICODES, SANS_V2_FEATURES)
    font = rename_glyphs(data, agl_names(data, {}))
    notes = {}
    move_points(font["glyf"]["a"], font["glyf"], A_POINTS_MOVED, A_DY)
    advance, lsb = font["hmtx"].metrics["n"]
    font["hmtx"].metrics["n"] = (advance + N_ADVANCE_DELTA, lsb)
    notes["kern"] = override_kern(font, KERN_LEFT, KERN_RIGHT, KERN_DELTA)
    notes["ss03_name_id"] = add_open_g(font)
    set_version(font, 4.1, "CRVN")
    pin_timestamps(font)
    return load(to_bytes(font)), notes


# ---------------------------------------------------------------- CFF


def to_cff(ttf):
    """Same font with CFF outlines (cubic, decomposed) and post format 3."""
    glyph_set = ttf.getGlyphSet()
    order = ttf.getGlyphOrder()
    advances = ttf["hmtx"].metrics
    charstrings = {}
    for name in order:
        pen = T2CharStringPen(advances[name][0], glyph_set)
        glyph_set[name].draw(pen)
        charstrings[name] = pen.getCharString()

    fb = FontBuilder(ttf["head"].unitsPerEm, isTTF=False)
    fb.setupGlyphOrder(order)
    names = ttf["name"]
    info = {
        "FullName": names.getDebugName(4),
        "FamilyName": names.getDebugName(1),
        "Weight": names.getDebugName(2),
        "version": "%.3f" % ttf["head"].fontRevision,
    }
    fb.setupCFF(names.getDebugName(6), info, charstrings, {})
    metrics = {}
    for name in order:
        bounds = charstrings[name].calcBounds(charstrings)
        metrics[name] = (advances[name][0], bounds[0] if bounds else 0)
    fb.setupHorizontalMetrics(metrics)
    fb.setupMaxp()
    for tag in ["head", "hhea", "cmap", "OS/2", "name", "GDEF", "GSUB", "GPOS", "STAT"]:
        if tag in ttf:
            fb.font[tag] = copy.deepcopy(ttf[tag])
    post = copy.deepcopy(ttf["post"])
    post.formatType = 3.0
    for attr in ("extraNames", "mapping", "glyphOrder"):
        if hasattr(post, attr):
            delattr(post, attr)
    fb.font["post"] = post
    fb.font.recalcTimestamp = False
    return load(to_bytes(fb.font))


# ---------------------------------------------------------------- family


def build_plain(path, unicodes, edit_a):
    """Basic Latin subset of a third-party face, optionally with `A` edited.
    Hinting is dropped to keep the collections small."""
    data = subset(path, unicodes, PLAIN_FEATURES, hinting=False)
    font = rename_glyphs(data, agl_names(data, {}))
    if edit_a:
        move_points(font["glyf"]["A"], font["glyf"], 2, A_DY)
    pin_timestamps(font)
    return to_bytes(font)


def write_ttc(path, faces):
    collection = TTCollection()
    collection.fonts = [load(face) for face in faces]
    buf = io.BytesIO()
    collection.save(buf, shareTables=True)
    write(path, buf.getvalue())


# ---------------------------------------------------------------- variable


def build_var(src, version):
    path = os.path.join(src, "InterVariable.ttf")
    data = subset(path, [ord(c) for c in VAR_TEXT], PLAIN_FEATURES)
    font = rename_glyphs(data, agl_names(data, {}))
    if len(data) > 60_000:
        font = instancer.instantiateVariableFont(font, {"opsz": None})
    if version == 2:
        fvar = font["fvar"]
        keep = []
        for instance in fvar.instances:
            name = font["name"].getDebugName(instance.subfamilyNameID)
            if name == "Thin":
                continue
            if name == "ExtraLight":
                instance.subfamilyNameID = font["name"].addName(
                    "UltraLight", platforms=((3, 1, 0x409),)
                )
            keep.append(instance)
        fvar.instances = keep
        points = len(font["glyf"]["o"].coordinates)
        for variation in font["gvar"].variations["o"]:
            coords = variation.coordinates
            for i in range(min(points, len(coords))):
                if coords[i] is not None:
                    x, y = coords[i]
                    coords[i] = (round(x * GVAR_SCALE), round(y * GVAR_SCALE))
    pin_timestamps(font)
    return to_bytes(font)


# ---------------------------------------------------------------- UFO


def ufo_sources(src):
    """Quadratic `a`, `acutecomb` and the aacute accent offset from Inter."""
    path = os.path.join(src, "Inter-Regular.ttf")
    font = TTFont(path, recalcTimestamp=False)
    glyph_set = font.getGlyphSet()
    a = RecordingPointPen()
    glyph_set["uni0061"].drawPoints(a)
    decomposed = DecomposingRecordingPen(glyph_set)
    glyph_set["uni0301"].draw(decomposed)
    acute = RecordingPointPen()
    decomposed.replay(SegmentToPointPen(acute))
    # Inter builds aacute from the spacing acute (uni00B4); uni0301 is that
    # same outline shifted left, so the combining-mark offset is the sum.
    accent = [c for c in font["glyf"]["uni00E1"].components if c.glyphName == "uni00B4"][0]
    shift = font["glyf"]["uni0301"].components[0].x
    return {
        "a": (a.value, font["hmtx"]["uni0061"][0]),
        "acutecomb": (acute.value, font["hmtx"]["uni0301"][0]),
        "accent_offset": (accent.x - shift, accent.y),
    }


def edit_recording(value, count, dy):
    """Copy of recorded point-pen calls with the first `count` points moved."""
    out = []
    moved = 0
    contour = 0
    for method, args, kwargs in value:
        if method == "endPath":
            contour += 1
        if method == "addPoint" and contour == 0 and moved < count:
            (x, y), rest = args[0], args[1:]
            args = ((x, y + dy),) + tuple(rest)
            moved += 1
        out.append((method, args, kwargs))
    return out


def replay(value):
    def draw(pen):
        for method, args, kwargs in value:
            getattr(pen, method)(*args, **kwargs)

    return draw


def glif(name, width, unicodes, draw, anchors=()):
    glyph = SimpleNamespace(width=width, unicodes=list(unicodes), anchors=list(anchors))
    return writeGlyphToString(name, glyph, draw, formatVersion=2)


def write_ufo(out, src_glyphs, cubic_o, version):
    root = os.path.join(out, "ufo-v%d" % version)
    shutil.rmtree(root, ignore_errors=True)
    a_value, a_width = src_glyphs["a"]
    acute_value, acute_width = src_glyphs["acutecomb"]
    dx, dy = src_glyphs["accent_offset"]
    anchors = []
    if version == 2:
        a_value = edit_recording(a_value, A_POINTS_MOVED, A_DY)
        anchors = [{"name": "top", "x": a_width // 2, "y": 1536}]
        dx, dy = dx + 24, dy + 16
    o_value, o_width = cubic_o

    def aacute(pen):
        pen.addComponent("a", (1, 0, 0, 1, 0, 0))
        pen.addComponent("acutecomb", (1, 0, 0, 1, dx, dy))

    glyphs = {
        "a": glif("a", a_width, [0x61], replay(a_value), anchors),
        "aacute": glif("aacute", a_width, [0xE1], aacute),
        "acutecomb": glif("acutecomb", acute_width, [0x301], replay(acute_value)),
        "o": glif("o", o_width, [0x6F], replay(o_value)),
    }
    contents = {name: name + ".glif" for name in glyphs}
    for name, text in glyphs.items():
        write(os.path.join(root, "glyphs", name + ".glif"), text)
    write(os.path.join(root, "glyphs", "contents.plist"), plist(contents))
    write(os.path.join(root, "metainfo.plist"), plist({"creator": "org.corvene.fixtures", "formatVersion": 3}))
    write(os.path.join(root, "layercontents.plist"), plist([["public.default", "glyphs"]]))


def plist(value):
    return plistlib.dumps(value, sort_keys=True).decode("utf-8")


def cubic_glyph(otf, name):
    recording = RecordingPointPen()
    otf.getGlyphSet()[name].draw(SegmentToPointPen(recording))
    return recording.value, otf["hmtx"][name][0]


# ---------------------------------------------------------------- Glyphs


class Raw(str):
    """Emitted verbatim in OpenStep output (node tuples, hex unicodes)."""


def quote(text):
    if text and (text[0].isalpha() or text[0] in "_.") and all(c.isalnum() or c in "_." for c in text):
        return text
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'


def openstep(value):
    if isinstance(value, dict):
        body = "".join("%s = %s;\n" % (quote(k), openstep(v)) for k, v in value.items())
        return "{\n" + body + "}"
    if isinstance(value, list):
        if not value:
            return "(\n)"
        return "(\n" + ",\n".join(openstep(v) for v in value) + "\n)"
    if isinstance(value, Raw):
        return str(value)
    if isinstance(value, int):
        return str(value)
    return quote(value)


class CubicRecorder(BasePen):
    """Segments with quadratics converted to cubics, coordinates rounded."""

    def __init__(self, glyph_set):
        super().__init__(glyph_set)
        self.contours = []

    def _moveTo(self, pt):
        self.contours.append([("move", pt)])

    def _lineTo(self, pt):
        self.contours[-1].append(("line", pt))

    def _curveToOne(self, p1, p2, p3):
        self.contours[-1].append(("curve", p1, p2, p3))

    def _closePath(self):
        pass

    def _endPath(self):
        pass


def rnd(pt):
    return (int(math.floor(pt[0] + 0.5)), int(math.floor(pt[1] + 0.5)))


def contour_nodes(contour):
    """Glyphs node list: start point last, `l`/`o`/`c` types."""
    start = rnd(contour[0][1])
    nodes = []
    for seg in contour[1:]:
        if seg[0] == "line":
            nodes.append([rnd(seg[1]), "l"])
        else:
            nodes += [[rnd(seg[1]), "o"], [rnd(seg[2]), "o"], [rnd(seg[3]), "c"]]
    if not nodes or nodes[-1][0] != start:
        nodes.append([start, "l"])
    return nodes


def smooth_flags(nodes):
    """Indices of curve nodes whose two handles are collinear."""
    flags = set()
    count = len(nodes)
    for i, (pt, kind) in enumerate(nodes):
        prev_pt, prev_kind = nodes[i - 1]
        next_pt, next_kind = nodes[(i + 1) % count]
        if kind != "c" or prev_kind != "o" or next_kind != "o":
            continue
        ax, ay = pt[0] - prev_pt[0], pt[1] - prev_pt[1]
        bx, by = next_pt[0] - pt[0], next_pt[1] - pt[1]
        size = math.hypot(ax, ay) * math.hypot(bx, by)
        if size and abs(ax * by - ay * bx) <= 0.02 * size and ax * bx + ay * by > 0:
            flags.add(i)
    return flags


def master_outlines(src):
    """Cubic contours, widths and the aacute accent offset of Inter Variable
    at wght 400 and 700 (opsz 14), keyed by glyph name then weight."""
    path = os.path.join(src, "InterVariable.ttf")
    data = subset(path, [0x61, 0x62, 0x63, 0x6F, 0x301, 0xE1], [])
    outlines = {}
    for weight in (400, 700):
        font = instancer.instantiateVariableFont(load(data), {"wght": weight, "opsz": 14})
        glyph_set = font.getGlyphSet()
        for name in ["a", "b", "c", "o", "acutecomb"]:
            pen = CubicRecorder(glyph_set)
            glyph_set[name].draw(pen)
            outlines.setdefault(name, {})[weight] = (pen.contours, font["hmtx"][name][0])
        accent = [c for c in font["glyf"]["aacute"].components if c.glyphName == "acutecomb"][0]
        outlines.setdefault("aacute", {})[weight] = (accent.x, accent.y)
    return outlines


def g3_shapes(contours, smooth_from, edit=False):
    shapes = []
    for index, contour in enumerate(contours):
        nodes = contour_nodes(contour)
        smooth = smooth_flags(contour_nodes(smooth_from[index]))
        if edit and index == 0:
            for node in nodes[:A_POINTS_MOVED]:
                node[0] = (node[0][0], node[0][1] + A_DY)
        raw = []
        for i, ((x, y), kind) in enumerate(nodes):
            raw.append(Raw("(%d,%d,%s)" % (x, y, kind + ("s" if i in smooth else ""))))
        shapes.append({"closed": 1, "nodes": raw})
    return shapes


def glyphs3(outlines, version):
    masters = [("m01", "Regular", 400), ("m02", "Bold" if version == 1 else "Heavy", 700)]
    names = ["a", "aacute", "acutecomb", "b", "o"] if version == 1 else ["a", "aacute", "acutecomb", "c", "o"]
    unicodes = {"a": 0x61, "aacute": 0xE1, "acutecomb": 0x301, "b": 0x62, "c": 0x63, "o": 0x6F}
    glyphs = []
    for name in names:
        layers = []
        for master_id, _, weight in masters:
            if name == "aacute":
                width = outlines["a"][weight][1]
                pos = Raw("(%d,%d)" % outlines["aacute"][weight])
                shapes = [{"ref": "a"}, {"pos": pos, "ref": "acutecomb"}]
            else:
                contours, width = outlines[name][weight]
                edit = version == 2 and name == "a" and weight == 700
                shapes = g3_shapes(contours, outlines[name][400][0], edit)
            layers.append({"layerId": master_id, "shapes": shapes, "width": width})
        glyphs.append({
            "glyphname": name,
            "lastChange": "2024-01-01 00:00:00 +0000",
            "layers": layers,
            "unicode": unicodes[name],
        })
    metric_types = ["ascender", "cap height", "x-height", "baseline", "descender"]
    metric_values = [
        {"over": 16, "pos": 2728},
        {"over": 16, "pos": 2048},
        {"over": 16, "pos": 1536},
        {"over": -16},
        {"over": -16, "pos": -680},
    ]
    doc = {
        ".appVersion": "3260",
        ".formatVersion": 3,
        "axes": [{"name": "Weight", "tag": "wght"}],
        "date": "2024-01-01 00:00:00 +0000",
        "familyName": "Corvene Sample",
        "features": [{"code": "sub a acutecomb by aacute;", "tag": "liga"}],
        "fontMaster": [
            {"axesValues": [weight], "id": master_id, "metricValues": metric_values, "name": name}
            for master_id, name, weight in masters
        ],
        "glyphs": glyphs,
        "kerningLTR": {"m01": {"a": {"o": -20 if version == 1 else -35}}},
        "metrics": [{"type": t} for t in metric_types],
        "unitsPerEm": 2816,
        "versionMajor": version,
        "versionMinor": 0,
    }
    return openstep(doc) + "\n"


def glyphs2(outlines):
    kinds = {"l": "LINE", "o": "OFFCURVE", "c": "CURVE"}
    glyphs = []
    for name, code in [("a", "0061"), ("aacute", "00E1"), ("acutecomb", "0301"), ("o", "006F")]:
        if name == "aacute":
            width = outlines["a"][400][1]
            layer = {
                "components": [
                    {"name": "a"},
                    {"name": "acutecomb", "transform": "{1, 0, 0, 1, %d, %d}" % outlines["aacute"][400]},
                ],
                "layerId": "m01",
                "width": width,
            }
        else:
            contours, width = outlines[name][400]
            paths = []
            for contour in contours:
                nodes = contour_nodes(contour)
                smooth = smooth_flags(nodes)
                text = [
                    "%d %d %s%s" % (x, y, kinds[kind], " SMOOTH" if i in smooth else "")
                    for i, ((x, y), kind) in enumerate(nodes)
                ]
                paths.append({"closed": 1, "nodes": text})
            layer = {"layerId": "m01", "paths": paths, "width": width}
        glyphs.append({"glyphname": name, "layers": [layer], "unicode": Raw(code)})
    doc = {
        ".appVersion": "1342",
        "date": "2024-01-01 00:00:00 +0000",
        "familyName": "Corvene Sample G2",
        "features": [{"code": "sub a acutecomb by aacute;", "name": "liga"}],
        "fontMaster": [{
            "ascender": 2728,
            "capHeight": 2048,
            "descender": -680,
            "id": "m01",
            "weightValue": 400,
            "xHeight": 1536,
        }],
        "glyphs": glyphs,
        "kerning": {"m01": {"a": {"o": -20}}},
        "unitsPerEm": 2816,
        "versionMajor": 1,
        "versionMinor": 0,
    }
    return openstep(doc) + "\n"


# ---------------------------------------------------------------- designspace


def designspace(version):
    """Designspace 5.0; rule conditions are in design (mapped) units."""
    wght_max = 900 if version == 1 else 1000
    maps = [(100, 20), (400, 80), (700, 140), (900, 180)]
    if version == 2:
        maps.append((1000, 200))
    instances = [("Regular", 400), ("Medium", 500), ("Bold", 700)]
    if version == 2:
        instances = [("Regular", 400), ("Bold", 700), ("Black", 900)]
    rule_min = 120 if version == 1 else 130  # user 600 / 650

    lines = [
        "<?xml version='1.0' encoding='UTF-8'?>",
        '<designspace format="5.0">',
        "  <axes>",
        '    <axis tag="wght" name="Weight" minimum="100" maximum="%d" default="400">' % wght_max,
    ]
    lines += ['      <map input="%d" output="%d"/>' % pair for pair in maps]
    lines += [
        "    </axis>",
        '    <axis tag="opsz" name="Optical size" values="14 32" default="14"/>',
        "  </axes>",
        "  <rules>",
        '    <rule name="heavy dollar">',
        "      <conditionset>",
        '        <condition name="Weight" minimum="%d" maximum="%d"/>' % (rule_min, dict(maps)[wght_max]),
        "      </conditionset>",
        '      <sub name="dollar" with="dollar.rvrn"/>',
        "    </rule>",
        "  </rules>",
        "  <sources>",
    ]
    for style, design in [("Regular", 80), ("Bold", 140)]:
        lines += [
            '    <source filename="Sample-%s.ufo" name="Sample %s" familyname="Corvene Sample" stylename="%s">'
            % (style, style, style),
            "      <location>",
            '        <dimension name="Weight" xvalue="%d"/>' % design,
            '        <dimension name="Optical size" xvalue="14"/>',
            "      </location>",
            "    </source>",
        ]
    lines += ["  </sources>", "  <instances>"]
    for style, weight in instances:
        lines += [
            '    <instance name="Sample %s" familyname="Corvene Sample" stylename="%s" filename="instances/Sample-%s.ufo">'
            % (style, style, style),
            "      <location>",
            '        <dimension name="Weight" uservalue="%d"/>' % weight,
            '        <dimension name="Optical size" uservalue="14"/>',
            "      </location>",
            "    </instance>",
        ]
    lines += ["  </instances>", "</designspace>", ""]
    return "\n".join(lines)


# ---------------------------------------------------------------- README


README = """# Font diff fixtures

Generated by `tools/fonts/make_fixtures.py` from fonts bundled with the
cosmic-text 0.19.0 and ttf-parser 0.25.1 crates. Do not edit by hand:
change the script and rerun it (the output is byte-for-byte reproducible).

```
python3 tools/fonts/make_fixtures.py \\
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cosmic-text-0.19.0/fonts \\
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ttf-parser-0.25.1/tests/fonts \\
  crates/corvene-fonts/tests/fixtures
```

## Sources and licenses

| Fixture | Source | License |
| --- | --- | --- |
| `sans-*`, `family-*.ttc` face 1 | Inter Regular (subset, modified) | OFL-1.1, `LICENSES/Inter-LICENSE` |
| `family-*.ttc` face 2 | Noto Sans Regular (subset, modified) | OFL-1.1, `LICENSES/NotoSans-LICENSE` |
| `family-v2.ttc` face 3 | Fira Mono Medium (subset) | OFL-1.1, `LICENSES/FiraMono-LICENSE` |
| `var-*.ttf` | Inter Variable (subset, modified) | OFL-1.1, `LICENSES/Inter-LICENSE` |
| `ufo-*`, `sample-*.glyphs` | outlines from Inter Regular / Inter Variable | OFL-1.1, `LICENSES/Inter-LICENSE` |
| `colr.ttf` | ttf-parser `tests/fonts/colr_1.ttf`, unchanged | Apache-2.0, `LICENSES/colr_1_LICENSE` |
| `family-*.designspace` | hand-written | same as the repository |

None of the OFL fonts declare a Reserved Font Name.

## Fixtures and what differs between v1 and v2

- `sans-v1.ttf` / `sans-v2.ttf` (TrueType, post format 2 names). Inter
  Regular subset to U+0020-007E plus U+00E9, U+00DF, U+2013, U+2019 with
  features kern liga calt ss01 ss02 case tnum frac ccmp mark mkmk (those
  present). v2:
  - `a`: points 0-3 of the first contour moved up 20 units.
  - `n`: advance width +12.
  - added `Euro` (U+20AC) and `Omega` (U+03A9).
  - removed `asciitilde` (U+007E); `asciitilde.case` goes with it.
  - kerning `T` + `o`: XAdvance {kern_old} -> {kern_new} (a format 1
    exception subtable put in front of the class-based kerning).
  - GSUB: `ss02` removed, `ss01` kept, `ss03` added (`g -> g.ss03`, new
    glyph with a 30 unit deeper descender, FeatureParams UI name
    "Open g" at name ID {ss03_name_id}). Dropping ss02 also drops the
    glyphs only it reached ({ss02_glyphs}) and its UI name (ID 257).
  - name ID 5 "Version 4.000" -> "Version 4.100" (ID 3 follows),
    head.fontRevision 4.0 -> 4.1 (stored as 16.16 fixed, reads back as
    4.100006), OS/2 achVendID RSMS -> CRVN (usWeightClass unchanged).
  - `uni00DF` renamed to `germandbls` (same outline and codepoint).
- `sans-v1.woff` (WOFF 1.0 of sans-v1) and `sans-v2.woff2` (WOFF2 of
  sans-v2, glyf/loca transformed).
- `sans-cff-v1.otf` / `sans-cff-v2.otf`: sans v1/v2 with CFF outlines
  (quadratics converted to cubics, components decomposed, post format 3).
  Same differences as the TrueType pair; the CFF charset keeps glyph
  names, so the `uni00DF` -> `germandbls` rename shows up here too.
- `family-v1.ttc`: sans-v1 + Noto Sans (Basic Latin).
  `family-v2.ttc`: sans-v2 + Noto Sans with `A` points 0-1 moved up 20
  units + new third face Fira Mono Medium (Basic Latin). Tables shared
  where identical.
- `var-v1.ttf` / `var-v2.ttf`: Inter Variable subset to "AHOVaehnost0123 "
  (fvar, gvar, avar, STAT, HVAR, MVAR kept). v2 removes the Thin named
  instance, renames ExtraLight to UltraLight (new name record), scales
  the gvar deltas of `o` by 1.2 (default outline identical; phantom
  points untouched). Axes, names and version otherwise unchanged.
- `colr.ttf`: COLRv1 test font, no v2.
- `ufo-v1/`, `ufo-v2/`: UFO 3 layer `glyphs/` with `a` (quadratic
  `qcurve` points), `acutecomb`, `aacute` (components `a` +
  `acutecomb`) and `o` (cubic `curve` points from the CFF outlines). v2
  moves points 0-3 of `a` up 20 units, adds anchor `top` to `a` and
  moves the `acutecomb` component of `aacute` by (+24, +16).
- `sample-v1.glyphs` / `sample-v2.glyphs`: Glyphs 3, masters Regular
  (wght 400) and Bold (wght 700) from Inter Variable, glyphs `a`, `aacute`
  (components), `acutecomb`, `b`, `o`, kerning `a`/`o`, a `liga` feature.
  v2 moves nodes 0-3 of `a` up 20 units in the Bold master only, adds
  `c`, removes `b`, changes the `a`/`o` kerning -20 -> -35 and renames
  the Bold master (same id `m02`) to Heavy.
- `sample-g2-v1.glyphs`: Glyphs 2 format (string nodes, `paths`,
  `components`, hex `unicode`), one master. No v2.
- `family-v1.designspace` / `family-v2.designspace`: format 5.0, axes
  wght (100-900 with a map) and discrete opsz (14, 32), sources Regular
  and Bold, instances Regular/Medium/Bold, rule `dollar -> dollar.rvrn`.
  v2: wght maximum 1000 (plus a map point), instance Medium removed and
  Black added, rule condition 120-180 -> 130-200 (design units, user
  600-900 -> 650-1000). Source files are references only and do not
  exist.
"""


# ---------------------------------------------------------------- main


def main(argv):
    if len(argv) != 4:
        raise SystemExit(__doc__)
    cosmic, ttf_parser, out = argv[1:]
    # Fira Mono's OS/2 v3 sets fsSelection bits fontTools warns about.
    logging.getLogger("fontTools").setLevel(logging.ERROR)
    os.makedirs(out, exist_ok=True)

    sans1, _ = build_sans(cosmic, 1)
    sans2, notes = build_sans(cosmic, 2)
    sans1_bytes, sans2_bytes = to_bytes(sans1), to_bytes(sans2)
    write(os.path.join(out, "sans-v1.ttf"), sans1_bytes)
    write(os.path.join(out, "sans-v2.ttf"), sans2_bytes)

    for data, flavor, name in [(sans1_bytes, "woff", "sans-v1.woff"), (sans2_bytes, "woff2", "sans-v2.woff2")]:
        font = load(data)
        font.flavor = flavor
        write(os.path.join(out, name), to_bytes(font))

    cff1, cff2 = to_cff(sans1), to_cff(sans2)
    write(os.path.join(out, "sans-cff-v1.otf"), to_bytes(cff1))
    write(os.path.join(out, "sans-cff-v2.otf"), to_bytes(cff2))

    noto = os.path.join(cosmic, "NotoSans-Regular.ttf")
    fira = os.path.join(cosmic, "FiraMono-Medium.ttf")
    write_ttc(os.path.join(out, "family-v1.ttc"), [sans1_bytes, build_plain(noto, BASIC_LATIN, False)])
    write_ttc(
        os.path.join(out, "family-v2.ttc"),
        [sans2_bytes, build_plain(noto, BASIC_LATIN, True), build_plain(fira, BASIC_LATIN, False)],
    )

    write(os.path.join(out, "var-v1.ttf"), build_var(cosmic, 1))
    write(os.path.join(out, "var-v2.ttf"), build_var(cosmic, 2))

    shutil.copyfile(os.path.join(ttf_parser, "colr_1.ttf"), os.path.join(out, "colr.ttf"))

    src_glyphs = ufo_sources(cosmic)
    cubic_o = cubic_glyph(cff1, "o")
    write_ufo(out, src_glyphs, cubic_o, 1)
    write_ufo(out, src_glyphs, cubic_o, 2)

    outlines = master_outlines(cosmic)
    write(os.path.join(out, "sample-v1.glyphs"), glyphs3(outlines, 1))
    write(os.path.join(out, "sample-v2.glyphs"), glyphs3(outlines, 2))
    write(os.path.join(out, "sample-g2-v1.glyphs"), glyphs2(outlines))

    write(os.path.join(out, "family-v1.designspace"), designspace(1))
    write(os.path.join(out, "family-v2.designspace"), designspace(2))

    licenses = os.path.join(out, "LICENSES")
    os.makedirs(licenses, exist_ok=True)
    for name in ["Inter-LICENSE", "NotoSans-LICENSE", "FiraMono-LICENSE"]:
        shutil.copyfile(os.path.join(cosmic, name), os.path.join(licenses, name))
    shutil.copyfile(os.path.join(ttf_parser, "colr_1_LICENSE"), os.path.join(licenses, "colr_1_LICENSE"))

    kern_old, kern_new = notes["kern"]
    gone = set(sans1.getGlyphOrder()) - set(sans2.getGlyphOrder())
    gone -= {"asciitilde", "asciitilde.case", "uni00DF"}
    readme = README.format(
        kern_old=kern_old,
        kern_new=kern_new,
        ss03_name_id=notes["ss03_name_id"],
        ss02_glyphs=", ".join("`%s`" % name for name in sorted(gone)),
    )
    write(os.path.join(out, "README.md"), readme)


if __name__ == "__main__":
    main(sys.argv)
