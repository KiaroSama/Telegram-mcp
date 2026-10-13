# Fidelity checks
text_fidelity is the exact entity anchor; cleaned text is presentation. Offsets
count UTF-16, not Unicode characters. A + non-BMP face + B: A0, face1 length2,
B3. Insertion before B shifts from unit3, not Python character2. Keep graphemes.
ID5934007978150595964 remains a string. Validate bounded ranges, complete fields,
Telegram-allowed nesting and unchanged links/spoilers/quotes. Never discard an
entity merely because emoji is added.
Rich blocks are separate content, not entities. Copy can preserve what ordinary
send cannot. Account entitlement and recipient/client support need evidence.

Visual record: ID, rendered frames, context, shape/color/motion. Fallback glyph
and rankings are not verdicts. Blank frame0 can be valid. Context-neutral text-
color preview does not prove exact surrounding color. Readback: exact text/full
entities/IDs, media/rich structure, topic/reply/schedule and visual comparison.

## Typography and exactness record
Exact string includes line breaks, whitespace, ZWNJ/ZWJ and RTL; normalization is a
change, not cleanup. Store each entity's type/offset/length and fields: URL, mention
identity, pre language, custom document ID or formatted-date properties as present.
Entity ranges index the exact string; supported nesting must survive composition.
Compare three axes independently: stored data | viewed native render | effect/state.
A font request maps to supported bold/italic/underline/strike/code/pre/quote/spoiler
only if that matches the requested appearance. Ordinary messages cannot select
arbitrary font family/size/color; CSS belongs to Mini Apps. Client theme/text size
can alter render without altering entities. Preserve exact links, not just labels.
Button icons are style.icon_document_id documents; label glyphs have no message entity mapping.
Use actual visible rendered frames for custom-emoji choice, then contextual output;
record missing rendering/native support as untested, not guessed equivalence.

Sources: https://core.telegram.org/api/entities,
https://core.telegram.org/bots/api#messageentity,
https://core.telegram.org/api/custom-emoji.
