# lightair-inochi2d

Lightair's built-in runtime for Inochi2D and nijilive puppets. It's the `inox2d` crate of
**Inox2D**, a Rust implementation of Inochi2D by the Inochi2D project
(https://github.com/Inochi2D/inox2d), taken at commit `d4dd9dd` (version 0.3.0) under the
BSD 2-Clause licence (`LICENSE`, which keeps its copyright notice) and built into Lightair.

Changes from that commit:

- MeshGroups (Inochi2D 0.8), parameters applied at zero, a public parameter list and part
  iterator, glam 0.30.
- nijilive puppets: PathDeformers, DynamicComposites, Mask nodes, welding.
- Renamed from `inox2d` to `lightair-inochi2d`, and not published. Only the library crate is
  here: not upstream's OpenGL renderer or examples.

`README.md` is upstream's, as it was at that commit.
