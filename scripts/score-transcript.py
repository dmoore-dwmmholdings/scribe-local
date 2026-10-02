#!/usr/bin/env python3
"""Score a pipeline transcript against a reference transcript of the same audio.

    score-transcript.py --ref meeting_otter.ai.txt --hyp dump.json [--json]

The hypothesis is the JSON written by `transcript_check` with
TRANSCRIPT_CHECK_DUMP. The reference is a transcript exported from a
transcription service as plain text: blocks of a speaker line ending in a
timestamp ("Jane Doe 1:23" or "Speaker 2 1:02:03"), then the words, then a
blank line. That is the format Otter.ai exports.

Both are reduced to lowercase words without punctuation, and filler sounds are
dropped from both, since services disagree about writing them. The two word
sequences are then aligned, and every aligned pair yields both scores:

- word error rate: substitutions, deletions and insertions over reference words.
- speaker accuracy: the share of aligned words credited to the right person,
  after the best one-to-one matching of hypothesis speakers to reference
  speakers. One-to-one, so splitting a person into two clusters, or merging two
  people into one, costs what it should.

A reference's unnamed speakers ("Speaker 2") are the service's own clusters it
could not put a name to. "named" accuracy repeats the score over only the
words the reference put a name on, which is the firmer half of the reference.

A reference transcript is not ground truth: it has its own errors, so these are
measures of agreement, and the ceiling is below 100%. They are for comparing
one pipeline change against another on the same recording.

Needs rapidfuzz and scipy (`pip install rapidfuzz scipy`).
"""

import argparse
import json
import re
import sys
from collections import Counter, defaultdict

import numpy as np
from rapidfuzz.distance import Levenshtein
from scipy.optimize import linear_sum_assignment

HEADER = re.compile(r"^(?P<name>.+?)\s+(?P<ts>\d{1,2}:\d{2}(?::\d{2})?)\s*$")
UNNAMED = re.compile(r"^Speaker \d+$")
FILLERS = {"um", "uh", "uhm", "umm", "uhh", "mm", "mmm", "hmm", "hm", "mhm", "erm", "er", "ah"}


def words_of(text):
    """Lowercase words, hyphens split, punctuation dropped, fillers removed."""
    out = []
    for tok in re.split(r"[\s\-–—/]+", text.lower()):
        tok = re.sub(r"[^a-z0-9']", "", tok).strip("'")
        if tok and tok not in FILLERS:
            out.append(tok)
    return out


def parse_reference(path):
    """[(word, speaker)] in order, from a speaker-line / text / blank-line export."""
    words, speaker = [], None
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("Transcribed by"):
            continue
        m = HEADER.match(line)
        if m:
            speaker = m.group("name")
            continue
        if speaker is None:
            continue
        words.extend((w, speaker) for w in words_of(line))
    return words


def parse_hypothesis(path):
    """[(word, speaker)] in order, from a transcript_check dump."""
    dump = json.load(open(path))
    words = []
    for w in dump["words"]:
        spk = w.get("spk")
        label = f"S{spk}" if spk is not None else None
        words.extend((t, label) for t in words_of(w["w"]))
    return words, dump


def score(ref, hyp):
    ref_tokens = [w for w, _ in ref]
    hyp_tokens = [w for w, _ in hyp]
    ops = Levenshtein.opcodes(ref_tokens, hyp_tokens)

    subs = dels = ins = 0
    pairs = []  # (ref index, hyp index) for words aligned to each other
    for op in ops:
        n_ref, n_hyp = op.src_end - op.src_start, op.dest_end - op.dest_start
        if op.tag == "equal":
            pairs.extend(zip(range(op.src_start, op.src_end), range(op.dest_start, op.dest_end)))
        elif op.tag == "replace":
            subs += min(n_ref, n_hyp)
            dels += max(0, n_ref - n_hyp)
            ins += max(0, n_hyp - n_ref)
            pairs.extend(zip(range(op.src_start, op.src_end), range(op.dest_start, op.dest_end)))
        elif op.tag == "delete":
            dels += n_ref
        elif op.tag == "insert":
            ins += n_hyp

    n_ref = len(ref_tokens)
    wer = (subs + dels + ins) / max(1, n_ref)

    # Speaker confusion over aligned words.
    conf = defaultdict(Counter)
    unlabelled = 0
    for i, j in pairs:
        r, h = ref[i][1], hyp[j][1]
        if h is None:
            unlabelled += 1
            continue
        conf[r][h] += 1
    ref_spk = sorted(conf)
    hyp_spk = sorted({h for c in conf.values() for h in c})
    matrix = np.array([[conf[r][h] for h in hyp_spk] for r in ref_spk], dtype=float)
    rows, cols = linear_sum_assignment(-matrix) if matrix.size else ([], [])
    mapping = {ref_spk[r]: hyp_spk[c] for r, c in zip(rows, cols)}

    total = len(pairs)
    correct = sum(conf[r][mapping[r]] for r in mapping)
    named = [r for r in ref_spk if not UNNAMED.match(r)]
    named_total = sum(sum(conf[r].values()) for r in named)
    named_correct = sum(conf[r][mapping[r]] for r in named if r in mapping)

    # Many-to-one views: how pure each cluster is, and how whole each person is.
    purity = sum(max(conf[r][h] for r in ref_spk) for h in hyp_spk) / max(1, total)
    coverage = sum(max(conf[r].values()) for r in ref_spk) / max(1, total)

    per_speaker = []
    for r in ref_spk:
        n = sum(conf[r].values())
        h = mapping.get(r)
        per_speaker.append({
            "ref": r,
            "words": n,
            "hyp": h,
            "accuracy": (conf[r][h] / n) if (h and n) else 0.0,
            "spread": conf[r].most_common(3),
        })
    per_speaker.sort(key=lambda s: -s["words"])

    return {
        "ref_words": n_ref,
        "hyp_words": len(hyp_tokens),
        "wer": wer,
        "subs": subs,
        "dels": dels,
        "ins": ins,
        "aligned": total,
        "unlabelled": unlabelled,
        "speaker_accuracy": correct / max(1, total),
        "named_accuracy": named_correct / max(1, named_total),
        "purity": purity,
        "coverage": coverage,
        "ref_speakers": len(ref_spk),
        "hyp_speakers": len(hyp_spk),
        "per_speaker": per_speaker,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ref", required=True)
    ap.add_argument("--hyp", required=True)
    ap.add_argument("--json", action="store_true", help="print the scores as one JSON object")
    ap.add_argument("--speakers", action="store_true", help="list every reference speaker")
    args = ap.parse_args()

    ref = parse_reference(args.ref)
    hyp, dump = parse_hypothesis(args.hyp)
    if not ref or not hyp:
        sys.exit("empty reference or hypothesis")
    s = score(ref, hyp)

    if args.json:
        s.pop("per_speaker")
        print(json.dumps(s))
        return
    print(f"word error rate   {100 * s['wer']:.1f}%  "
          f"(sub {s['subs']}, del {s['dels']}, ins {s['ins']} over {s['ref_words']} reference words)")
    print(f"speaker accuracy  {100 * s['speaker_accuracy']:.1f}%  over {s['aligned']} aligned words")
    print(f"  named only      {100 * s['named_accuracy']:.1f}%")
    print(f"  purity          {100 * s['purity']:.1f}%  (each cluster's majority person)")
    print(f"  coverage        {100 * s['coverage']:.1f}%  (each person's majority cluster)")
    print(f"speakers          {s['hyp_speakers']} found, {s['ref_speakers']} in the reference")
    if s["unlabelled"]:
        print(f"no speaker        {s['unlabelled']} aligned words")
    if args.speakers:
        for p in s["per_speaker"]:
            spread = ", ".join(f"{h}:{n}" for h, n in p["spread"])
            print(f"  {p['ref']:<22} {p['words']:>6} words -> {p['hyp'] or '-':<4} "
                  f"{100 * p['accuracy']:5.1f}%   [{spread}]")


if __name__ == "__main__":
    main()
