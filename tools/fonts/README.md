# Font tools

- `make_fixtures.py` rebuilds `crates/corvene-fonts/tests/fixtures` (see
  the README there for the command and what each fixture holds).
- `crates/corvene-fonts/src/blocks.rs` is generated from fontTools'
  `unicodedata.Blocks`. To refresh it after a Unicode update:

```
python3 - <<'PY'
from fontTools.unicodedata import Blocks
R, V = Blocks.RANGES, Blocks.VALUES
for i, start in enumerate(R):
    if V[i] != "No_Block":
        end = R[i + 1] - 1 if i + 1 < len(R) else 0x10FFFF
        print(f'    (0x{start:04X}, 0x{end:04X}, "{V[i]}"),')
PY
```

and paste the rows into `BLOCKS`.
