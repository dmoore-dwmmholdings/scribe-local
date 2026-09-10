"""Stand in for the LLM, so the stages that call one can be exercised without one.

Two pipeline stages talk to a language model: `merge` asks it to fix misheard
proper nouns in the transcript, and `summarize` asks it for a summary. Both are
written to degrade rather than fail when it is unreachable, which is easy to
check by pointing them at a closed port — and much harder to check in the other
direction, since it needs a model.

This answers both the Ollama (`/api/chat`) and OpenAI (`/v1/chat/completions`)
shapes, and applies a fixed glossary of corrections read out of the prompt. That
covers everything except the model's judgement: whether the request is built
right, the reply parsed, and the corrections applied.

    python3 scripts/stub-llm.py 8799 [mode]

    glossary   fix known mis-hearings (the default, and the useful one)
    summarise  ignore the instructions and return summaries instead
    truncate   replace every line with "ok"
    garbage    reply with something that is not JSON

The last three exist because `merge` claims a flaky model cannot corrupt a
transcript, and that claim is worth testing. With any of them the transcript
should come back exactly as the recogniser produced it.

    [llm]
    base_url = "http://127.0.0.1:8799"
    provider = "ollama"
    summarize_model = "stub"
"""

import json
import re
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

# Mis-hearings this project's own fixtures produce, and what they should be.
GLOSSARY = {
    "shivon": "Siobhan", "syabhan": "Siobhan", "cuba needs": "Kubernetes",
    "rotor": "rota", "efa": "Aoife", "eva": "Aoife", "nayam": "Niamh",
    "ian": "Eoghan", "quiksilver": "Quicksilver", "bellweather": "Bellwether",
    "mugule": "module",
}

MODE = "glossary"


def numbered_lines(prompt):
    for line in prompt.splitlines():
        m = re.match(r"^\s*(\d+):\s*(.+)$", line)
        if m:
            yield int(m.group(1)), m.group(2)


def corrections(prompt):
    if MODE == "garbage":
        return "not json at all"
    if MODE == "summarise":
        return [{"i": i, "text": "The team discussed on-call scheduling."}
                for i, _ in numbered_lines(prompt)]
    if MODE == "truncate":
        return [{"i": i, "text": "ok"} for i, _ in numbered_lines(prompt)]

    out = []
    for i, text in numbered_lines(prompt):
        fixed = text
        for wrong, right in GLOSSARY.items():
            fixed = re.sub(re.escape(wrong), right, fixed, flags=re.I)
        if fixed != text:
            out.append({"i": i, "text": fixed})
    return out


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        # Both providers' "are you there" probes.
        self._send({"models": [{"name": "stub"}], "data": [{"id": "stub"}]})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"{}")
        prompt = "\n".join(m.get("content", "") for m in body.get("messages", []))

        if "numbered transcript lines" in prompt:
            reply = corrections(prompt)
            reply = reply if isinstance(reply, str) else json.dumps(reply)
        else:
            # A summary request. Enough shape for the stage to store something.
            reply = json.dumps({
                "summary": "A stand-in summary.",
                "topics": ["scheduling"],
                "action_items": [],
                "decisions": [],
            })
        # Record what each stage actually sends, so a caller can check that
        # speaker labels survive as far as the model — which is the whole point
        # of diarizing before summarising.
        kind = ("correct" if "numbered transcript lines" in prompt
                else "condense" if "part " in prompt and "of a meeting transcript" in prompt
                else "summarise")
        # Labels at the start of a transcript line, which is how the pipeline
        # carries who said what into a prompt.
        speakers = sorted(set(re.findall(r"^([A-Z][\w .'-]{0,24}?): ", prompt, flags=re.M))
                          - {"Transcript"})
        roster = re.search(r"The people speaking are: ([^.]+)\.", prompt)
        sys.stderr.write(
            f"[stub-llm] {kind}: {len(prompt)} chars"
            f", labelled lines from: {', '.join(speakers) if speakers else '(none)'}"
            f", roster: {roster.group(1) if roster else '(none)'}\n"
        )

        if self.path.endswith("/chat/completions"):
            self._send({"choices": [{"message": {"content": reply}}]})
        else:
            self._send({"message": {"content": reply}})

    def _send(self, obj):
        data = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8799
    if len(sys.argv) > 2:
        MODE = sys.argv[2]
    sys.stderr.write(f"[stub-llm] listening on {port} in {MODE} mode\n")
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
