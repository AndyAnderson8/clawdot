"""A free stand-in for the language model: an OpenAI-compatible /chat/completions that answers with a fixed
sentence quoting the question. For rehearsals that must not spend money.

    python fake_llm.py 8083        # then use llm.url http://127.0.0.1:8083/v1
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        req = json.loads(self.rfile.read(length))
        asked = [m for m in req["messages"] if m["role"] == "user"][-1]["content"]
        print("language model was asked:", asked, flush=True)
        text = "You said: " + str(asked)[:120] + ". This is the local test reply."
        usage = {"prompt_tokens": 1, "completion_tokens": 1}
        if req.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            for word in text.split(" "):
                chunk = {"choices": [{"index": 0, "delta": {"content": word + " "}}]}
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
            done = {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}
            self.wfile.write(("data: " + json.dumps(done) + "\n\ndata: [DONE]\n\n").encode())
        else:
            body = json.dumps({
                "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
                "usage": usage,
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)


HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
