import pytest

from kobo_bridge import markup


def test_blocks_and_spans():
    blocks = markup.parse(
        "# Today\n\nLine one\nline two **bold**\n\n- a\n- b\n\n> quoted\n\n---\n\n"
        "![map](https://example.com/m.png)\n")
    assert [b["t"] for b in blocks] == ["h", "p", "ul", "q", "hr", "img"]
    assert blocks[1]["spans"] == [{"s": "Line one line two "}, {"s": "bold", "b": True}]
    assert blocks[2]["items"] == [[{"s": "a"}], [{"s": "b"}]]
    assert blocks[5] == {"t": "img", "alt": "map", "url": "https://example.com/m.png"}


def test_deeper_headings_become_level_two():
    assert markup.parse("### Small")[0]["level"] == 2


def test_http_images_stay_text():
    blocks = markup.parse("![x](http://example.com/x.png)")
    assert blocks[0]["t"] == "p"


def test_limits():
    with pytest.raises(markup.MarkupError):
        markup.parse("x" * (markup.MAX_BODY_CHARS + 1))
    with pytest.raises(markup.MarkupError):
        markup.parse("\n\n".join("p" for _ in range(markup.MAX_BLOCKS + 1)))
    with pytest.raises(markup.MarkupError):
        markup.parse(123)
