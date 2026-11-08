"""A scripted model server for the halti eval tasks (tests only): it plays one of scripts.json's solutions per task,
each command as a shell call and then the answer. The task is the one whose prompt the first user message ends with;
the solution is the kind named in DIR/round (oracle, alt0, bad1, ...), or the oracle when the task has no such one.
Any other prompt gets "ready". Each request's task and kind go to DIR/log.

  python3 -I stub.py DIR SCRIPTS_JSON TASKS_DIR    (writes the port it listens on to DIR/port)
"""
import http.server, json, os, socketserver, sys

d, scripts_file, tasks_dir = sys.argv[1:4]
scripts = json.load(open(scripts_file))["scripts"]
prompts = {}
for t in scripts:
    with open(os.path.join(tasks_dir, t, "prompt.txt")) as f:
        prompts[t] = f.read().strip()


def chunk(delta, finish=None):
    return "data: " + json.dumps({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
                                  "object": "chat.completion.chunk"}) + "\n\n"


class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        msgs = body["messages"]
        first = next(m["content"] for m in msgs if m["role"] == "user").strip()
        task = next((t for t, p in prompts.items() if first.endswith(p)), None)
        finish = "stop"
        if task is None:
            delta = {"role": "assistant", "content": "ready"}
            kind = "-"
        else:
            try:
                want = open(os.path.join(d, "round")).read().strip()
            except OSError:
                want = "oracle"
            pick = [s for s in scripts[task] if s["kind"] == want] or [s for s in scripts[task] if s["kind"] == "oracle"]
            s = pick[0]
            kind = s["kind"]
            done = sum(1 for m in msgs if m["role"] == "tool")
            if done < len(s["cmds"]):
                call = {"index": 0, "id": "c%d" % done, "type": "function",
                        "function": {"name": "shell", "arguments": json.dumps({"command": s["cmds"][done]})}}
                delta, finish = {"role": "assistant", "tool_calls": [call]}, "tool_calls"
            else:
                delta = {"role": "assistant", "content": s["answer"]}
        with open(os.path.join(d, "log"), "a") as f:
            f.write("%s\t%s\n" % (task or "-", kind))
        sse = (chunk(delta) + chunk({}, finish) + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(sse)))
        self.end_headers()
        self.wfile.write(sse)

    def do_GET(self):
        if self.path != "/props":
            self.send_response(404)
            self.end_headers()
            return
        b = json.dumps({"model_path": "/models/stub.gguf", "build_info": "b1-stub"}, separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def log_message(self, *a):
        pass


s = socketserver.TCPServer(("127.0.0.1", 0), H)
open(os.path.join(d, "port"), "w").write(str(s.server_address[1]))
s.serve_forever()
