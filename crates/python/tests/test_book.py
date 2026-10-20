"""The book's Python chapter, run: each of its examples does what the chapter says."""

import pathlib
import re

CHAPTER = pathlib.Path(__file__).resolve().parents[3] / "docs" / "book" / "src" / "python.md"


def test_the_python_chapter_runs():
    text = CHAPTER.read_text(encoding="utf-8")
    blocks = re.findall(r"^```py\n(.*?)^```$", text, flags=re.S | re.M)
    assert len(blocks) >= 7
    # The examples follow on from each other, as a reader runs them.
    namespace = {}
    for block in blocks:
        exec(compile(block, str(CHAPTER), "exec"), namespace)
