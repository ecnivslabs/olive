import ast, sys, os, json, importlib.util
from collections import defaultdict, Counter

def find_pyi(module_name):
    parts = module_name.split(".")
    try:
        spec = importlib.util.find_spec(module_name)
        if spec and spec.origin:
            base = os.path.dirname(spec.origin)
            for candidate in (
                os.path.join(base, "__init__.pyi"),
                os.path.join(os.path.dirname(base), parts[-1] + ".pyi"),
            ):
                if os.path.exists(candidate):
                    return candidate
    except Exception:
        pass
    for path in sys.path:
        candidates = [
            os.path.join(path, module_name + "-stubs", "__init__.pyi"),
            os.path.join(path, "py" + module_name + "-stubs", module_name, "__init__.pyi"),
        ]
        if len(parts) > 1:
            candidates.append(
                os.path.join(path, parts[0] + "-stubs", *parts[1:], "__init__.pyi")
            )
        for c in candidates:
            if c and os.path.exists(c):
                return c
    return None

def node_name(node):
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    if isinstance(node, ast.Subscript):
        if isinstance(node.value, ast.Name) and node.value.id == "Optional":
            return node_name(node.slice)
    return None

def extract(path):
    with open(path, encoding="utf-8", errors="ignore") as f:
        src = f.read()
    try:
        tree = ast.parse(src)
    except SyntaxError:
        return {"types": [], "aliases": {}, "fns": {}, "fields": {}, "methods": {}}

    classes = set()
    typevars = {}           # name -> [constraint class names]
    alias_to_canonical = {} # fvec3 -> vec3
    canonical_to_preferred = {}  # vec3 -> vec3, mat4x4 -> mat4

    for node in tree.body:
        if isinstance(node, ast.ClassDef):
            classes.add(node.name)
            canonical_to_preferred[node.name] = node.name

    for node in tree.body:
        if isinstance(node, ast.Assign):
            for t in node.targets:
                if not isinstance(t, ast.Name):
                    continue
                val = node.value
                if isinstance(val, ast.Name) and val.id in classes and t.id not in classes:
                    canonical = val.id
                    alias = t.id
                    alias_to_canonical[alias] = canonical
                    current = canonical_to_preferred.get(canonical, canonical)
                    if len(alias) < len(current):
                        canonical_to_preferred[canonical] = alias
                elif isinstance(val, ast.Call):
                    fn = val.func
                    is_tv = (isinstance(fn, ast.Name) and fn.id == "TypeVar") or \
                            (isinstance(fn, ast.Attribute) and fn.attr == "TypeVar")
                    if is_tv:
                        cs = [node_name(a) for a in val.args[1:] if node_name(a) in classes]
                        typevars[t.id] = cs

    def preferred(name):
        if name is None:
            return None
        canonical = alias_to_canonical.get(name, name)
        return canonical_to_preferred.get(canonical, canonical)

    PRIMITIVES = {"float": "float", "int": "int", "bool": "bool", "str": "str", "None": "None"}

    def resolve_type(name):
        if name is None:
            return None
        if name in classes or name in alias_to_canonical:
            return preferred(name)
        if name in typevars:
            cs = typevars[name]
            return preferred(cs[0]) if cs else None
        return PRIMITIVES.get(name)

    def raw_anns(node):
        """Return raw annotation name strings (unresolved) for non-self/cls args."""
        all_args = list(getattr(node.args, "posonlyargs", [])) + list(node.args.args)
        result = []
        for arg in all_args:
            if arg.arg in ("self", "cls"):
                continue
            result.append(node_name(arg.annotation) if arg.annotation else None)
        return result

    def make_overloads(param_anns, ret_ann):
        """
        Expand TypeVar-constrained signatures into one overload per constraint.
        Returns a list of {"params": [...], "ret": str} dicts.
        """
        tv_in_sig = set()
        for ann in param_anns:
            if ann and ann in typevars and typevars[ann]:
                tv_in_sig.add(ann)
        if ret_ann and ret_ann in typevars and typevars[ret_ann]:
            tv_in_sig.add(ret_ann)

        if not tv_in_sig:
            params = [resolve_type(a) or "PyObject" for a in param_anns]
            ret = resolve_type(ret_ann) or "PyObject"
            return [{"params": params, "ret": ret}]

        # Collect constraints from the first TypeVar with constraints
        constraints = []
        for tv in tv_in_sig:
            cs = typevars.get(tv, [])
            if cs:
                constraints = cs
                break

        if not constraints:
            params = [resolve_type(a) or "PyObject" for a in param_anns]
            ret = resolve_type(ret_ann) or "PyObject"
            return [{"params": params, "ret": ret}]

        result = []
        for c in constraints:
            pref_c = preferred(c)

            def sub(ann, pref_c=pref_c):
                if ann and ann in typevars:
                    return pref_c
                return resolve_type(ann) or "PyObject"

            result.append({
                "params": [sub(a) for a in param_anns],
                "ret": sub(ret_ann) if ret_ann is not None else "PyObject",
            })
        return result

    def disambiguate(raw):
        """
        Deduplicate overloads by (params_tuple) key.
        Distinct param signatures coexist; when a single param sig has multiple returns,
        use the most common return (mode) as the resolved type.
        """
        sig_rets = {}   # params_tuple -> list of ret strings
        order = []
        for o in raw:
            key = tuple(o["params"])
            if key not in sig_rets:
                sig_rets[key] = []
                order.append(key)
            sig_rets[key].append(o["ret"])

        resolved = []
        for key in order:
            params_list = list(key)
            rets = sig_rets[key]
            unique_rets = set(rets)
            if len(unique_rets) == 1:
                ret = rets[0]
            else:
                ret = Counter(rets).most_common(1)[0][0]
            resolved.append({"params": params_list, "ret": ret})
        return resolved

    # Module-level functions (with TypeVar expansion)
    raw_fns = defaultdict(list)
    for node in tree.body:
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        param_anns = raw_anns(node)
        ret_ann = node_name(node.returns) if node.returns else None
        for overload in make_overloads(param_anns, ret_ann):
            raw_fns[node.name].append(overload)

    # Class constructors + per-class fields and methods (with TypeVar expansion)
    raw_methods = defaultdict(lambda: defaultdict(list))
    fields = {}

    for node in tree.body:
        if not isinstance(node, ast.ClassDef):
            continue
        cls_name = preferred(node.name)
        cls_fields = {}
        for item in node.body:
            if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                ft = resolve_type(node_name(item.annotation))
                if ft:
                    cls_fields[item.target.id] = ft
            elif isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                param_anns = raw_anns(item)
                ret_ann = node_name(item.returns) if item.returns else None
                if item.name == "__init__":
                    for overload in make_overloads(param_anns, None):
                        raw_fns[cls_name].append({"params": overload["params"], "ret": cls_name})
                else:
                    for overload in make_overloads(param_anns, ret_ann):
                        raw_methods[cls_name][item.name].append(overload)
        if cls_fields:
            fields[cls_name] = cls_fields

    fns = {name: disambiguate(overloads) for name, overloads in raw_fns.items()}
    methods = {
        cls: {m: disambiguate(sigs) for m, sigs in mmap.items()}
        for cls, mmap in raw_methods.items()
    }

    all_types = sorted({preferred(c) for c in classes})

    aliases = {}
    for c in classes:
        p = preferred(c)
        if c != p:
            aliases[c] = p
    for alias, canonical in alias_to_canonical.items():
        aliases[alias] = preferred(canonical)

    return {"types": all_types, "aliases": aliases, "fns": fns,
            "fields": fields, "methods": methods}

module = os.environ.get("OLIVE_PYI_MODULE", "")
path = find_pyi(module)
if path:
    print(json.dumps(extract(path)))
else:
    print(json.dumps({"types": [], "aliases": {}, "fns": {}, "fields": {}, "methods": {}}))
