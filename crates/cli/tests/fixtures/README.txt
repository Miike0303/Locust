synthetic-ascii.ttf is generated entirely by Locust's hand-crafted
font_validation::build_minimal_ascii_font() test helper. It contains a synthetic
ASCII cmap and metrics only, no usable glyph outlines or third-party font data.
It tests binary validation and patch transactions, not visual rendering.

c127-scripts.rpa is a synthetic RPA-3.0 archive with key 0, one member
scripts/story.rpy, and a zlib-compressed protocol-2 pickle index. Its entire
script is: label start: followed by the dialogue "Hello from the archive."
It proves Direct injection creates a loose Ren'Py overlay and backup restore
removes that unchanged overlay without removing unrelated mods. It contains
no third-party game data. The server restore test uses the same fixture.
