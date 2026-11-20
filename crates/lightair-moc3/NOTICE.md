# lightair-moc3

Lightair's built-in runtime for `.moc3` avatar models. It's **Mocari**, a pure Rust runtime by
Eatgrapes (https://github.com/Eatgrapes/Mocari), taken at commit `29a4792` (version 0.4.1) under
the MIT licence (`LICENSE`, which keeps its copyright notice) and built into Lightair.

Changes from that commit:

- Renamed from `mocari` to `lightair-moc3`, and not published.
- Only the library is here: `src/`. Upstream's sample models (`assets/`), examples and
  integration tests aren't, so the unit tests that read the sample models don't run here.

`README.md` is upstream's, as it was at that commit.
