"""Injects noise then real incidents, and writes the ground truth to out/truth.jsonl."""
import json
import time
import urllib.request

APP = "http://fakeapp:9100"


def set_gauge(metric, labels, value):
    req = urllib.request.Request(APP, data=json.dumps({"metric": metric, "labels": labels, "value": value}).encode(),
                                 method="POST", headers={"Content-Type": "application/json"})
    urllib.request.urlopen(req).close()


def truth(summary, page):
    with open("/out/truth.jsonl", "a") as f:
        f.write(json.dumps({"summary": summary, "page": page, "at": int(time.time())}) + "\n")


def main():
    open("/out/truth.jsonl", "w").close()
    set_gauge("demo_cpu", {"instance": "worker-1", "env": "prod"}, 10)
    set_gauge("demo_error_ratio", {"instance": "api-1", "env": "prod"}, 0.0)
    set_gauge("demo_disk", {"instance": "db-staging", "env": "staging"}, 40)
    time.sleep(20)
    for _ in range(6):  # noise: short CPU flaps build a "usually resolves in a minute" history
        set_gauge("demo_cpu", {"instance": "worker-1", "env": "prod"}, 95)
        time.sleep(20)
        set_gauge("demo_cpu", {"instance": "worker-1", "env": "prod"}, 10)
        time.sleep(25)
    truth("CPU above 90% on worker-1", 0)
    set_gauge("demo_disk", {"instance": "db-staging", "env": "staging"}, 97)
    truth("Disk above 95% on db-staging", 0)
    time.sleep(20)
    set_gauge("demo_error_ratio", {"instance": "api-1", "env": "prod"}, 0.2)
    truth("5xx above 5% on api-1", 1)
    set_gauge("demo_cpu", {"instance": "worker-1", "env": "prod"}, 95)
    truth("CPU above 90% on worker-1 (stuck)", 1)
    time.sleep(240)
    print("chaos done", flush=True)


main()
