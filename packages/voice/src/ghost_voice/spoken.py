"""What reaches the voice is speech, not a document.

The model is told to answer in plain sentences, but tool results and app notices pull
it toward Markdown now and then (a Mobbin notice once put a ``---`` rule in a reply, which
Kokoro cannot say). This filter runs inside the TTS service on each chunk of text, after
sentence aggregation: it drops lines with nothing to say, unwraps links and emphasis, and
strips list and heading markers. It never rewrites words.
"""

from __future__ import annotations

import re

from pipecat.utils.text.base_text_filter import BaseTextFilter

_LINK = re.compile(r"\[([^\]]+)\]\([^)]*\)")  # [text](url) -> text
# **text** / __text__ -> text, only at word edges: x**2 and a_b__c are not emphasis
_STRONG = re.compile(r"(?<![\w*_])(\*\*|__)(?=\S)(.+?)(?<=\S)\1(?![\w*_])")
_CODE = re.compile(r"`+([^`]*)`+")  # `code` -> code
_MARKER = re.compile(r"^\s*(?:#{1,6}\s+|[-*+]\s+|\d+[.)]\s+|>\s+)")  # heading, bullet, quote
_SAYABLE = re.compile(r"[^\W_]", re.UNICODE)  # a letter or a digit


def spoken(text: str) -> str:
    """The sayable part of ``text``; "" when there is none."""
    if not text:
        return text
    lines = []
    for line in text.split("\n"):
        line = _LINK.sub(r"\1", line)
        line = _STRONG.sub(r"\2", line)
        line = _CODE.sub(r"\1", line)
        line = _MARKER.sub("", line)
        line = line.replace("|", " ")
        if not _SAYABLE.search(line):
            continue  # a rule, a blank, a row of pipes
        lines.append(line)
    out = "\n".join(lines)
    # Keep the chunk's trailing space: the aggregator joins chunks with it.
    if text.endswith(" ") and out and not out.endswith(" "):
        out += " "
    return out


class SpokenTextFilter(BaseTextFilter):
    """Pipecat text filter: ``spoken`` on every chunk the TTS service is about to say."""

    async def filter(self, text: str) -> str:
        return spoken(text)

    async def update_settings(self, settings) -> None:  # noqa: ANN001 - base signature
        pass

    async def handle_interruption(self) -> None:
        pass

    async def reset_interruption(self) -> None:
        pass
