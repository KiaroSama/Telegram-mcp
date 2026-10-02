# Qt ICC oracle fixtures

These are synthetic, non-sensitive image/profile fixtures, not account exports. See
`provenance.json` for exact upstream commits, construction and SHA256 digests.

- `.icc`: binary ICC profiles; never decode or transcode them as text.
- `.rgb.hex` / `.gray.hex`: UTF-8 ASCII hexadecimal transport of raw pixel buffers;
  `bytes.fromhex` reconstructs the bytes whose hashes are recorded in provenance.
- `adobe-thumbnail.source.jpg` / `adobe-thumbnail.expected.jpg`: a synthetic source
  and its complete 8x8 thumbnail from the exact patched Qt 5.15.19 image pipeline.

The native oracle uses QtCore/QtGui and the static JPEG plugin from the pinned Qt
source, Desktop patches 0020/0021, and mozjpeg 4.1.5/JPEG8. It runs without a GUI,
Telegram connection or credentials. Its default density is normalized to Desktop's
verified Windows 96 dpi before the writer runs. The RGB positive control matches
`5b3ee84dc8e3fee426431a1b46e3d7671df2c88ae71d14e35b7050c3727eeae0`.

CI reads the captured bytes; it does not build or run a native Qt oracle. Keep these
fixtures tracked with the tests. To refresh, rebuild the same pinned image engine,
verify the RGB control first, generate the synthetic inputs, compare decoded pixels,
emitted ICC and complete thumbnail bytes, then update the provenance hashes. Never
replace these with personal media or loosen assertions to accommodate a mismatch.
