"""Load and filter Russian/English sentence pairs with IPA transcriptions.

Provides `SentencePairs`, a small chainable loader over a JSON file of
records shaped like:

    {"id": 1, "ru": "...", "ipa": "[...]", "en": "..."}

and `parse_id_list`, which parses printer-style id specs like
"1,3,5-8" into an explicit list of integers for use with `.filter()`.

Typical usage (as a library, no CLI):

    from voynaimir.db.sentence_pairs import SentencePairs, parse_id_list

    pairs = SentencePairs("50_russian_english_ipa_words.json")
    ids = parse_id_list("1,3,5-8")
    for pair in pairs.filter(ids):
        print(pair.ru, pair.en)

    SentencePairs(JSON_PATH).filter(parse_id_list("1,3,5-8")).each(
        lambda pair: print(pair.ru, pair.ipa, pair.en)
    )

    uv run python -c "
    from voynaimir.db.sentence_pairs import SentencePairs, parse_id_list
    SentencePairs('src/voynaimir/db/50_russian_english_ipa_words.json').filter(parse_id_list('1,3,5-8')).each(
        lambda p: print(f'{p.id}\\n{p.ru}\\n{p.ipa}\\n{p.en}\\n{p.words}\\n\\n')
    )
"

No external dependencies beyond the standard library.
"""

import copy
import json
import os
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass
from typing import Any, Self


def parse_id_list(spec: str) -> list[int]:
    """
    Parse a printer-style id list, e.g. "1,3,5-8" -> [1, 3, 5, 6, 7, 8].
    """
    result: list[int] = []
    for part in spec.split(","):
        if "-" in part:
            start, end = part.split("-")
            result.extend(range(int(start), int(end) + 1))
        else:
            result.append(int(part))
    return result


@dataclass(frozen=True, slots=True)
class SentencePair:
    """A single Russian/English sentence with its IPA transcription and id."""

    id: int
    ru: str
    ipa: str
    en: str
    words: str


class SentencePairs:
    """Chainable, lazily-loaded view over a JSON file of `SentencePair` records.

    `filter()` returns a new `SentencePairs` narrowed to the given ids;
    the underlying JSON is only read (and cached) on first iteration.
    """

    def __init__(self, path: str | os.PathLike[str]):
        self.path = path
        self._wanted_ids: set[int] | None = None
        self._cache: list[SentencePair] | None = None

    def _load_all(self) -> list[SentencePair]:
        if self._cache is None:
            with open(self.path, encoding="utf-8") as f:
                data = json.load(f)
            self._cache = [SentencePair(**item) for item in data]
        return self._cache

    def filter(self, ids: Iterable[int]) -> Self:
        """
        Filter by ID values (not positions).

        The numbers from parse_id_list(...) are treated as SentencePair.id values.
        Example:
          parse_id_list("1,3,5-8") → [1, 3, 5, 6, 7, 8]
          This will select all SentencePair objects whose .id is in {1,3,5,6,7,8}.

        Returns a new SentencePairs; the original is left unmodified.
        Chaining `.filter()` calls intersects the id sets.
        """
        id_set = set(ids)
        wanted = id_set if self._wanted_ids is None else self._wanted_ids & id_set
        return self._spawn(wanted)

    def _spawn(self, wanted_ids: set[int]) -> Self:
        clone = copy.copy(self)
        clone._wanted_ids = wanted_ids
        return clone

    def __iter__(self) -> Iterator[SentencePair]:
        items = self._load_all()
        if self._wanted_ids is not None:
            items = [item for item in items if item.id in self._wanted_ids]
        return iter(items)

    def each(self, fn: Callable[[SentencePair], Any]) -> None:
        """Run `fn` on each filtered SentencePair."""
        for obj in self:
            fn(obj)

    def to_list(self) -> list[SentencePair]:
        """Return filtered items as a list."""
        return list(self)
