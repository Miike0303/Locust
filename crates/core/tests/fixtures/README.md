proportional-latin.ttf is Locust-owned synthetic cmap/metric data, generated with
Python's standard library by `python -B crates/core/tests/fixtures/generate_proportional_font.py`.
It contains no third-party font data and no usable outlines; it is an arithmetic
fixture, never a rendering or engine-fit oracle. Generation is deterministic.

1000 units/em; ASCII U+0020..U+007E plus U+00E9 (é), U+00F1 (ñ), U+0100 (Ā),
and U+2014 (—). Advances: i=200, W=900, every other glyph=500 units.
At 20 px: iii=12 px, WWW=54 px, space=10 px; width 30 distinguishes the first
two strings despite their equal scalar counts. No kerning or shaping exists.

The separate crates/cli/tests/fixtures/synthetic-ascii.ttf is reused unchanged:
ASCII only, 1000 units/em, fixed 500-unit advances; Hola at 28 px is 56 px.
