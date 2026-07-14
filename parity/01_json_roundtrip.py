import json

d = json.loads('{"count": 1, "score": 2}')
d["count"] = d["count"] + 1
print(json.dumps(d))
