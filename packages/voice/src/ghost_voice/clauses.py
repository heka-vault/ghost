"""Hand Kokoro a clause at a time instead of a sentence.

Kokoro renders a whole chunk before any of it plays, about 0.23 s per second of speech
(3 words 0.3 s, 15 words 1.1 s, 30 words 2.4 s), so a reply that opens with a long
sentence was silent for seconds (docs/benchmarks/2026-10-19-latency.md). This aggregator
releases at the end of a sentence, as Pipecat's does, and also at a clause boundary
(a comma, colon, semicolon or dash followed by a space) once the pending text has
``min_words`` words. The punctuation stays with the clause, so Kokoro still reads its
intonation.

It only changes how text is chunked for synthesis. ``NarrationCut`` (before TTS) still
holds a short reply until the response ends and then releases it whole; from there it
reaches Kokoro clause by clause. What the assistant tap records and what the context gets
is unchanged: the tap sits before TTS, and the clauses concatenate back to the reply.
"""

from __future__ import annotations

from pipecat.utils.text.base_text_aggregator import Aggregation, AggregationType
from pipecat.utils.text.simple_text_aggregator import SimpleTextAggregator

CLAUSE_MARKS = frozenset(",;:—–")


class ClauseTextAggregator(SimpleTextAggregator):
    def __init__(self, *, min_words: int = 4, **kwargs) -> None:
        super().__init__(**kwargs)
        self._min_words = min_words

    async def _check_sentence_with_lookahead(self, char: str) -> Aggregation | None:
        sentence = await super()._check_sentence_with_lookahead(char)
        if sentence is not None or self._aggregation_type != AggregationType.SENTENCE:
            return sentence
        return self._clause(char)

    def _clause(self, char: str) -> Aggregation | None:
        # A boundary is a mark followed by whitespace: "Three are waiting, the ..." but
        # not "3,000" or "owner/app:142". A spaced hyphen counts as a dash.
        if not char.isspace() or len(self._text) < 2:
            return None
        prev = self._text[-2]
        dash = prev == "-" and len(self._text) >= 3 and self._text[-3] == " "
        if prev not in CLAUSE_MARKS and not dash:
            return None
        clause = self._text[:-1].strip()
        if len(clause.split()) < self._min_words:
            return None
        self._text = ""
        # Don't keep a pending sentence check from before the clause (e.g. "Dr. Smith,").
        self._lookahead_state = type(self._lookahead_state).IDLE
        return Aggregation(text=clause, type=AggregationType.SENTENCE)
