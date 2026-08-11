# /// script
# requires-python = ">=3.14"
# dependencies = []
# ///


import json
import os
import sys
from dataclasses import asdict, dataclass


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


def load_sentence_pairs(path: str | os.PathLike[str]) -> list[SentencePair]:
    """
    Load all `SentencePair` records from a JSON file.

    Each record in the file must contain the fields `id`, `ru`, `ipa`,
    `en`, and `words`.
    """
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    return [SentencePair(**item) for item in data]


def main(argv: list[str]) -> int:
    """
    CLI entry point: `<path> <id-spec>`.

    Loads records from `path`, keeps only those whose `id` appears in the
    parsed `id-spec` (e.g. "1,3,5-8"), and prints the resulting records to
    stdout as a JSON array.
    """
    if len(argv) != 2:
        print(f"usage: {os.path.basename(sys.argv[0])} <path> <id-spec>", file=sys.stderr)
        return 1

    path, id_spec = argv
    wanted_ids = set(parse_id_list(id_spec))
    pairs = load_sentence_pairs(path)
    selected = [p for p in pairs if p.id in wanted_ids]

    print(json.dumps([asdict(p) for p in selected], ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
