# Numbers

How keenwake was measured, and how to read the results. Every number here can be reproduced with the
command next to it.

## Corpus

A synthetic corpus of 210 firing alerts (15 scenarios, written by an LLM, labelled "should page"
or not). AUC is the chance that a random alert that should page scores above one that should not.

| Scorer | AUC | Reproduce |
|---|---|---|
| A 3-line rule, no model: "over 3x its median, or no history" + "is prod" | 0.931 | `scripts/cargo.sh test --test corpus` |
| Laya, local (`typed-decisions` @ `55cf4c4e`, RTX 3080) | 0.949 | `--ignored`, see [tests/corpus.rs](../tests/corpus.rs) |
| Jev, hosted (`jev-1.13.0`, 210 calls in 49 s) | 1.000 | same |

What this says, and what it does not:

- **History does most of the work.** The rule gets 14 of 15 scenarios right. It cannot tell "TLS
  certificate expires in 17 hours, renewal failed" from "expires in 22 days, renewal scheduled":
  their history is identical, only the text differs. Jev reads the text; the rule cannot.
- **Laya is barely above the rule.** Its value today is that no data leaves the machine, not
  accuracy. The demo shows the same thing on live alerts.
- **This is a ceiling, not a production result.** Each scenario has one label, which makes the task
  easier than real alerts. The rule was written after looking at the corpus, which flatters it.
  The number that counts is `keenwake report` on your own alerts after a week in observe.

## Demo, measured end to end

`demo/e2e.sh` in gate mode, one run per backend. A chaos script injects noise (six short CPU
flaps, a full disk on staging) then two real incidents (a 5xx spike, a CPU that stays pegged).

| Backend | Real incidents paged | Noise paged | Noise held for the digest | Backend cost |
|---|---|---|---|---|
| Jev | 2 of 2 | 1 | 6 | $0.0004 for 21 calls |
| Laya, on CPU | 2 of 2 | 7 | 0 | none, local |

With Jev, the one noisy page is the very first CPU flap: no history yet, so it pages, as it
should. The pegged CPU first looked like its usual flap and went to the digest, then paged once it
outlasted its usual duration. With Laya at the default thresholds, every flap paged: its scores
barely move with history (0.59 to 0.66 here), which matches its corpus result.

