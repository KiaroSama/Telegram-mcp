# Native visual and structured access

Structured Telegram data and rendered evidence answer different questions. The Rust
adapters preserve exact text, UTF-16 entity ranges, custom-emoji IDs, keyboard structure,
media metadata and unknown TL fields where the tool's documented result includes them.
The fallback glyph of a custom emoji does not identify its actual artwork.

## Images and previews

The native image pipeline decodes supported image types, bounds dimensions and pixel
allocations, preserves aspect ratio, avoids upscaling and produces actual PNG/JPEG/WebP
bytes. Contact sheets retain failed tiles and their labels rather than silently
truncating the requested set. Invalid images, unsupported formats and allocation bombs
are refused before expensive rendering.

Local source/destination paths remain subject to the permitted file-root policy.
Preview/download limits must be reported, and a partial or failed decode is not a
successful original-quality export. Metadata-only checks do not establish rendered
appearance.

## Animated stickers and custom emoji

TGS files are gzipped Lottie animations. Optional native rendering loads a trusted
rlottie shared library through the Rust adapter. Set `TELEGRAM_RLOTTIE_LIBRARY` to the
absolute `.so`, `.dylib` or `.dll` path for your installation. The loader also accepts
`RLOTTIE_LIBRARY` as a compatibility alias. The shared library is executable code;
choose it from the reviewed dependency build, not a chat-supplied path.

CI builds official Samsung/rlottie revision
`ea06d2f29ba01b8d06c00a838d107f5e484ae59b` externally on Linux, macOS and Windows.
Its renderer legs set `TELEGRAM_REQUIRE_LOTTIE=1`; the test must render distinct real
frames and fail when the native library is unavailable. Base legs separately exercise
behavior without the optional library. The repository does not vendor C++ source.

Native decoding runs behind explicit byte/pixel/frame/time budgets. Missing optional
rendering is reported or uses a documented thumbnail fallback; it must not be described
as a rendered animation. Keep structured document IDs as decimal strings so JSON clients
do not round them.

## Emoji Studio

The standalone native `emoji-studio` executable replaces the former skill scripts.
`sheet` produces labelled contact sheets, `gif` samples actual animation frames, and
`index`, `refresh` and `nearest` maintain and query visual fingerprints. Local pack
exports use `--packs-dir` or `TELEGRAM_MCP_EMOJI_PACKS`; `--cache` selects the local cache.
Incremental refresh checks source size/mtime and removes vanished pack-owned entries.
Similarity compares cropped, alpha-normalized silhouette, colour and structure; it is
visual ranking rather than a semantic description of the artwork.

`lines` inspects a message and `insert` composes an edit while preserving UTF-16 entity
boundaries. Remote operations use the authenticated shared MCP service selected with
`--url` and optional `--account`, including the normal owner-approval gate for edits.
`accuracy --catalog PATH` reads known original/republished pairs from a local SQLite
catalog without modifying it. Use `emoji-studio --help` and the subcommand help for
current bounds and output options. Release artifacts include this executable;
the server-only Docker image currently does not.

## Native capture

Telegram Desktop window capture is Windows-specific and uses native APIs. The code
validates process/window selection, method, format, region and allocation limits.
Other platforms report that the capture operation requires Windows. This platform
restriction does not prevent ordinary Telegram API tools or native builds on macOS/Linux.

A successful synthetic capture test is not proof that a particular user's Telegram
Desktop version, DPI setting, permissions or window state can be captured. Real rendered
proof requires a real approved window and must be reported separately from CI fixtures.

## Rich text and exports

The HTML renderer escapes markup and attributes, rejects unsafe link schemes and path
traversal, and preserves RTL joiners and compound emoji. Entity spans use Telegram's
UTF-16 offsets, including astral characters. Structured JSON retains raw TL data where
needed for fidelity rather than silently dropping new-layer fields.

Exports use bounded paging and truthful job lifecycle states. They protect existing
destinations and downloaded media paths. A readable HTML/JSON export is not proof of
byte-identical Telegram Desktop/Qt color management or mozjpeg thumbnails; remaining
ICC, scaling or byte-format differences must stay explicit in tests and the PR report.
See [migration limits](MIGRATION.md) for the current boundary and retained attribution.
