# Marked synthetic detector fixture

`synthetic-portrait.jpg` is a completely fictional adult generated with the built-in image generator on 2026-11-08. No real-person image, name, account or identity reference was supplied. It exists only for isolated detector/landmark and anonymous normalized-vector numerical consistency checks. Do not add it to a business media library, identity reference gallery, classification batch or account mapping.

`provenance.json` records the prompt, original generated PNG hash, final JPEG hash and encoding. JPEG conversion preserves the original 1024×1536 dimensions; benchmark CPU/GPU paths must consume these **same bytes**, not independently resize or re-encode their inputs. The final fixture is 482366 bytes, below the protocol limit.

The CPU-only smoke command requires explicit model and fixture hashes and can optionally compare CPU recognition vectors from existing FaceAnalysis against reconstructed JSON geometry on the same decoded raster. It records only aggregate vector dimensions/norms/differences, never a raw vector or identity lookup. Positive CUDA 640 consistency still requires a separate resource-admitted run; CPU success is not GPU evidence.
