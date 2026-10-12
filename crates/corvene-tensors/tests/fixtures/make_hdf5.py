"""Regenerates the HDF5 fixtures: python3 make_hdf5.py (needs h5py, numpy)."""
import os
import h5py
import numpy as np

here = os.path.dirname(os.path.abspath(__file__))

# Keras 2 style, default (earliest) file format: superblock v0, v1 object
# headers, symbol table groups, v3 layouts with v1 B-tree chunk indexes.
with h5py.File(os.path.join(here, "keras2.h5"), "w", track_order=False) as f:
    f.attrs["keras_version"] = np.bytes_(b"2.15.0")
    f.attrs["backend"] = np.bytes_(b"tensorflow")
    f.attrs["note"] = "variable length text"
    f.attrs["epochs"] = np.int64(12)
    f.attrs["lr"] = np.float32(0.5)
    mw = f.create_group("model_weights")
    mw.attrs["layer_names"] = np.array([b"dense", b"dense_1"])
    d = mw.create_group("dense")
    d.attrs["weight_names"] = np.array([b"dense/kernel:0", b"dense/bias:0"])
    dd = d.create_group("dense")
    dd.create_dataset("kernel:0", data=np.arange(12, dtype=np.float32).reshape(4, 3))
    dd.create_dataset("bias:0", data=np.array([0.5, -0.5, 1.0], dtype=np.float32))
    d1 = mw.create_group("dense_1").create_group("dense_1")
    d1.create_dataset("kernel:0", data=np.arange(70, dtype=np.float32).reshape(10, 7),
                      chunks=(4, 3), compression="gzip", shuffle=True)
    f.create_dataset("ints", data=np.arange(-3, 3, dtype=np.int16))
    f.create_dataset("flags", data=np.array([True, False, True]))
    f.create_dataset("big", data=np.array([1.5, -2.25], dtype=">f8"))
    f.create_dataset("half", data=np.array([1, 2, 3, 4], dtype=np.float16))
    f.create_dataset("empty", shape=(0, 3), dtype=np.float32)

# Newest file format: superblock v3, v2 object headers, dense link and
# attribute storage, v4 layouts (fixed array chunk index).
with h5py.File(os.path.join(here, "latest.h5"), "w", libver="latest") as f:
    g = f.create_group("many")
    for i in range(20):
        g.create_dataset(f"w{i:02}", data=np.full((2,), i, dtype=np.float32))
    for i in range(12):
        g.attrs[f"a{i:02}"] = np.int32(i)
    f.create_dataset("chunked", data=np.arange(24, dtype=np.float64).reshape(4, 6), chunks=(2, 4))
    f.create_dataset("deflated", data=np.arange(30, dtype=np.int32).reshape(5, 6), chunks=(5, 2),
                     compression="gzip")
    f.create_dataset("plain", data=np.linspace(-1, 1, 8, dtype=np.float32))
