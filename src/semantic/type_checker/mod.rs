mod expr;
mod patterns;
mod stmt;
mod unify;

use super::error::SemanticError;
use super::pyi::PyiInfo;
use super::types::Type;
use crate::parser::{Program, Stmt};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Clone, Debug)]
pub struct TraitDef {
    pub methods: Vec<(String, Type)>,
}

/// One function/method's overloads: each entry is (param types, return type).
type OverloadSet = Vec<(Vec<Type>, Type)>;
/// Function name → its overloads, within a single Python module or class.
type FnOverloads = HashMap<String, OverloadSet>;
/// Positional arity bounds `(min, max)` per overload of one function; `max` is
/// `None` for a variadic (`*args`) signature.
type ArityBounds = Vec<(usize, Option<usize>)>;
/// Module alias → function name → arity bounds of each overload.
type PyArityMap = HashMap<String, HashMap<String, ArityBounds>>;

/// Renders the set of accepted positional argument counts for an arity-mismatch
/// message, e.g. "exactly 2 argument(s)", "1 to 3 argument(s)", or "at least 1
/// argument(s)". Collapses every overload's bounds into one overall range.
fn describe_arity(arities: &[(usize, Option<usize>)]) -> String {
    let min = arities.iter().map(|(m, _)| *m).min().unwrap_or(0);
    let unbounded = arities.iter().any(|(_, mx)| mx.is_none());
    let max = if unbounded {
        None
    } else {
        arities.iter().filter_map(|(_, mx)| *mx).max()
    };
    match max {
        None => format!("at least {min} argument(s)"),
        Some(mx) if mx == min => format!("exactly {min} argument(s)"),
        Some(mx) => format!("{min} to {mx} argument(s)"),
    }
}

pub struct TypeChecker {
    pub(super) substitutions: HashMap<usize, Type>,
    pub expr_types: HashMap<usize, Type>,
    pub type_env: Vec<HashMap<String, Type>>,
    pub(super) current_return_type: Option<Type>,
    pub errors: Vec<SemanticError>,
    pub warnings: Vec<SemanticError>,
    pub(super) mut_env: Vec<HashMap<String, bool>>,
    pub field_types: HashMap<(String, String), Type>,
    pub enum_variants: HashMap<String, Vec<String>>,
    /// Enum name to its variants in tag order, each with its payload types. Used
    /// to render enum values by name when printing.
    pub enum_defs: HashMap<String, Vec<(String, Vec<Type>)>>,
    pub(super) current_struct: Option<String>,
    pub(super) async_depth: usize,
    pub(super) vararg_fns: HashSet<String>,
    /// Per fn, count of leading non-default params (call may omit trailing defaults).
    pub(super) fn_required_args: HashMap<String, usize>,
    pub struct_fields: HashMap<String, Vec<String>>,
    /// Per struct, the count of leading fields that have no default value and so
    /// must be supplied when constructing it positionally.
    pub struct_required_fields: HashMap<String, usize>,
    pub traits: HashMap<String, TraitDef>,
    pub(super) type_traits: HashSet<(String, String)>,
    pub(super) c_ffi_structs: HashSet<String>,
    pub(super) unsafe_depth: usize,
    pub ffi_fns: HashSet<String>,
    pub c_ffi_fns: HashSet<String>,
    pub(super) var_counter: usize,
    pub init_params: HashMap<String, Vec<String>>,
    pub expr_kwarg_maps: HashMap<usize, Vec<usize>>,
    // module alias → type_name → resolved type
    pub(super) py_module_types: HashMap<String, HashMap<String, Type>>,
    // module alias → fn_name → list of (param_types, return_type) overloads
    pub(super) py_module_fns: HashMap<String, FnOverloads>,
    // set of names bound via `import py "..." as <alias>`
    pub(super) py_aliases: HashSet<String>,
    // (module alias, class name) → field_name → type
    pub(super) py_class_fields: HashMap<(String, String), HashMap<String, Type>>,
    // (module alias, class name) → method_name → overloads
    pub(super) py_class_methods: HashMap<(String, String), FnOverloads>,
    // module alias → fn_name → positional arity bounds (min, max) per overload
    pub(super) py_fn_arity: PyArityMap,
    // aliases whose surface comes from an explicit `import py` stub block (a
    // closed contract), as opposed to best-effort `.pyi` introspection
    pub(super) py_explicit_modules: HashSet<String>,
    // module alias → real Python module name, for diagnostics
    pub(super) py_alias_module: HashMap<String, String>,
    // expected type for the next checked expression; consumed once in
    // `infer_expr`. Lets an annotation drive a collection literal.
    pub(super) expected: Option<Type>,
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut global_env = HashMap::default();

        let builtins = [
            (
                "print",
                Type::Fn(vec![Type::Any], Box::new(Type::Int), Vec::new()),
            ),
            (
                "str",
                Type::Fn(vec![Type::Any], Box::new(Type::Str), Vec::new()),
            ),
            (
                "int",
                Type::Fn(vec![Type::Any], Box::new(Type::Int), Vec::new()),
            ),
            (
                "i64",
                Type::Fn(vec![Type::Any], Box::new(Type::Int), Vec::new()),
            ),
            (
                "i32",
                Type::Fn(vec![Type::Any], Box::new(Type::I32), Vec::new()),
            ),
            (
                "i16",
                Type::Fn(vec![Type::Any], Box::new(Type::I16), Vec::new()),
            ),
            (
                "i8",
                Type::Fn(vec![Type::Any], Box::new(Type::I8), Vec::new()),
            ),
            (
                "u64",
                Type::Fn(vec![Type::Any], Box::new(Type::U64), Vec::new()),
            ),
            (
                "usize",
                Type::Fn(vec![Type::Any], Box::new(Type::Usize), Vec::new()),
            ),
            (
                "u32",
                Type::Fn(vec![Type::Any], Box::new(Type::U32), Vec::new()),
            ),
            (
                "u16",
                Type::Fn(vec![Type::Any], Box::new(Type::U16), Vec::new()),
            ),
            (
                "u8",
                Type::Fn(vec![Type::Any], Box::new(Type::U8), Vec::new()),
            ),
            (
                "float",
                Type::Fn(vec![Type::Any], Box::new(Type::Float), Vec::new()),
            ),
            (
                "f64",
                Type::Fn(vec![Type::Any], Box::new(Type::Float), Vec::new()),
            ),
            (
                "f32",
                Type::Fn(vec![Type::Any], Box::new(Type::F32), Vec::new()),
            ),
            (
                "bool",
                Type::Fn(vec![Type::Any], Box::new(Type::Bool), Vec::new()),
            ),
            (
                "type",
                Type::Fn(vec![Type::Any], Box::new(Type::Str), Vec::new()),
            ),
            (
                "len",
                Type::Fn(vec![Type::Any], Box::new(Type::Int), Vec::new()),
            ),
            (
                "slice",
                Type::Fn(
                    vec![Type::Any, Type::Int, Type::Int],
                    Box::new(Type::Any),
                    Vec::new(),
                ),
            ),
            (
                "list_new",
                Type::Fn(
                    vec![Type::Int],
                    Box::new(Type::List(Box::new(Type::Any))),
                    Vec::new(),
                ),
            ),
            (
                "bytes_new",
                Type::Fn(vec![Type::Int], Box::new(Type::Bytes), Vec::new()),
            ),
            (
                "bytes_push",
                Type::Fn(vec![Type::Any, Type::Int], Box::new(Type::Null), Vec::new()),
            ),
            (
                "bytes_push_u16_le",
                Type::Fn(vec![Type::Any, Type::Int], Box::new(Type::Null), Vec::new()),
            ),
            (
                "bytes_push_u32_le",
                Type::Fn(vec![Type::Any, Type::Int], Box::new(Type::Null), Vec::new()),
            ),
            (
                "list",
                Type::Fn(
                    vec![Type::Any],
                    Box::new(Type::List(Box::new(Type::Any))),
                    Vec::new(),
                ),
            ),
            (
                "dict",
                Type::Fn(
                    vec![Type::Any],
                    Box::new(Type::Dict(Box::new(Type::Str), Box::new(Type::Any))),
                    Vec::new(),
                ),
            ),
            (
                "__olive_async_file_read",
                Type::Fn(
                    vec![Type::Str],
                    Box::new(Type::Future(Box::new(Type::Str))),
                    Vec::new(),
                ),
            ),
            (
                "__olive_async_file_write",
                Type::Fn(
                    vec![Type::Str, Type::Str],
                    Box::new(Type::Future(Box::new(Type::Int))),
                    Vec::new(),
                ),
            ),
            (
                "__olive_gather",
                Type::Fn(
                    vec![Type::List(Box::new(Type::Future(Box::new(Type::Param(
                        "T".into(),
                    )))))],
                    Box::new(Type::Future(Box::new(Type::List(Box::new(Type::Param(
                        "T".into(),
                    )))))),
                    vec![Type::Param("T".into())],
                ),
            ),
            (
                "__olive_select",
                Type::Fn(
                    vec![Type::List(Box::new(Type::Future(Box::new(Type::Param(
                        "T".into(),
                    )))))],
                    Box::new(Type::Future(Box::new(Type::Param("T".into())))),
                    vec![Type::Param("T".into())],
                ),
            ),
            (
                "__olive_free_future",
                Type::Fn(vec![Type::Any], Box::new(Type::Int), Vec::new()),
            ),
            (
                "__olive_py_realize",
                Type::Fn(vec![Type::Any], Box::new(Type::PyObject), Vec::new()),
            ),
            (
                "realize",
                Type::Fn(vec![Type::Any], Box::new(Type::PyObject), Vec::new()),
            ),
            (
                "__olive_math_sin",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_cos",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_tan",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_asin",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_acos",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_atan",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_atan2",
                Type::Fn(
                    vec![Type::Float, Type::Float],
                    Box::new(Type::Float),
                    Vec::new(),
                ),
            ),
            (
                "__olive_math_log",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_log10",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_math_exp",
                Type::Fn(vec![Type::Float], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_random_seed",
                Type::Fn(vec![Type::Int], Box::new(Type::Null), Vec::new()),
            ),
            (
                "__olive_random_get",
                Type::Fn(vec![], Box::new(Type::Float), Vec::new()),
            ),
            (
                "__olive_random_int",
                Type::Fn(vec![Type::Int, Type::Int], Box::new(Type::Int), Vec::new()),
            ),
            (
                "__olive_net_tcp_connect",
                Type::Fn(vec![Type::Str], Box::new(Type::Int), Vec::new()),
            ),
            (
                "__olive_net_tcp_send",
                Type::Fn(vec![Type::Int, Type::Str], Box::new(Type::Int), Vec::new()),
            ),
            (
                "__olive_net_tcp_recv",
                Type::Fn(vec![Type::Int, Type::Int], Box::new(Type::Str), Vec::new()),
            ),
            (
                "__olive_net_tcp_close",
                Type::Fn(vec![Type::Int], Box::new(Type::Null), Vec::new()),
            ),
            (
                "__olive_http_get",
                Type::Fn(vec![Type::Str], Box::new(Type::Str), Vec::new()),
            ),
            (
                "__olive_http_post",
                Type::Fn(vec![Type::Str, Type::Str], Box::new(Type::Str), Vec::new()),
            ),
            (
                "__olive_spawn_task",
                Type::Fn(
                    vec![Type::Any],
                    Box::new(Type::Future(Box::new(Type::Any))),
                    Vec::new(),
                ),
            ),
            (
                "ffi_errno",
                Type::Fn(vec![], Box::new(Type::Int), Vec::new()),
            ),
        ];

        let mut traits = HashMap::default();
        traits.insert(
            "Error".to_string(),
            TraitDef {
                methods: Vec::new(),
            },
        );
        let mut type_traits = HashSet::default();
        type_traits.insert(("Error".to_string(), "Error".to_string()));

        for (name, ty) in builtins {
            global_env.insert(name.to_string(), ty);
        }

        // `Error` is a built-in single-variant enum `Error(str)`. Modelling it as
        // an enum (rather than a struct) lets `T | Error` results be discriminated
        // by `match`, since enums carry a runtime type tag and structs do not.
        global_env.insert("Error".to_string(), Type::Enum("Error".to_string(), vec![]));
        global_env.insert(
            "Error::Error".to_string(),
            Type::Fn(
                vec![Type::Str],
                Box::new(Type::Enum("Error".to_string(), vec![])),
                vec![],
            ),
        );

        let mut enum_variants: HashMap<String, Vec<String>> = HashMap::default();
        enum_variants.insert("Error".to_string(), vec!["Error".to_string()]);

        Self {
            substitutions: HashMap::default(),
            expr_types: HashMap::default(),
            type_env: vec![global_env],
            current_return_type: None,
            errors: Vec::new(),
            warnings: Vec::new(),
            mut_env: vec![HashMap::default()],
            field_types: HashMap::default(),
            enum_variants,
            enum_defs: HashMap::default(),
            current_struct: None,
            async_depth: 0,
            vararg_fns: HashSet::default(),
            fn_required_args: HashMap::default(),
            struct_fields: HashMap::default(),
            struct_required_fields: HashMap::default(),
            traits,
            type_traits: HashSet::default(),
            c_ffi_structs: HashSet::default(),
            unsafe_depth: 0,
            ffi_fns: HashSet::default(),
            c_ffi_fns: HashSet::default(),
            var_counter: 0,
            init_params: HashMap::default(),
            expr_kwarg_maps: HashMap::default(),
            py_module_types: HashMap::default(),
            py_module_fns: HashMap::default(),
            py_aliases: HashSet::default(),
            py_class_fields: HashMap::default(),
            py_class_methods: HashMap::default(),
            py_fn_arity: HashMap::default(),
            py_explicit_modules: HashSet::default(),
            py_alias_module: HashMap::default(),
            expected: None,
        }
    }

    pub(super) fn fresh_var_id(&mut self) -> usize {
        let id = self.var_counter;
        self.var_counter += 1;
        id
    }

    pub(super) fn fresh_var(&mut self) -> Type {
        Type::Var(self.fresh_var_id())
    }

    pub(super) fn enter_scope(&mut self) {
        self.type_env.push(HashMap::default());
        self.mut_env.push(HashMap::default());
    }

    pub(super) fn leave_scope(&mut self) {
        self.type_env.pop();
        self.mut_env.pop();
    }

    pub(super) fn define_type(&mut self, name: &str, ty: Type, is_mut: bool) {
        if let Some(scope) = self.type_env.last_mut() {
            scope.insert(name.to_string(), ty);
        }
        if let Some(scope) = self.mut_env.last_mut() {
            scope.insert(name.to_string(), is_mut);
        }
    }

    pub(super) fn lookup_type(&self, name: &str) -> Option<Type> {
        for scope in self.type_env.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    /// Checks `module.attr(...)` against an explicit `import py` stub, a closed
    /// surface the programmer wrote, reporting an unknown attribute and a
    /// positional argument count no overload accepts. `.pyi`-introspected modules
    /// are skipped: a stub can't enumerate a module's full surface (C extensions,
    /// `__getattr__`, re-exports), so flagging an absent name there false-positives
    /// on packages like `pygame`. Argument types are not enforced, since Python
    /// coerces at runtime, leaving existence and arity as the only sound checks.
    pub(super) fn check_py_call(
        &mut self,
        module: &str,
        attr: &str,
        arg_count: usize,
        all_positional: bool,
        span: crate::span::Span,
    ) {
        if !self.py_explicit_modules.contains(module) {
            return;
        }
        let display = self
            .py_alias_module
            .get(module)
            .map(String::as_str)
            .unwrap_or(module);
        let known_fn = self
            .py_module_fns
            .get(module)
            .is_some_and(|m| m.contains_key(attr));
        let known_type = self
            .py_module_types
            .get(module)
            .is_some_and(|m| m.contains_key(attr));

        if !known_fn && !known_type {
            let mut names: Vec<String> = Vec::new();
            if let Some(m) = self.py_module_fns.get(module) {
                names.extend(m.keys().cloned());
            }
            if let Some(m) = self.py_module_types.get(module) {
                names.extend(m.keys().cloned());
            }
            let suggestions = super::suggest::closest_n(attr, names.iter().map(String::as_str), 3);
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0601",
                        format!("Python module `{display}` has no attribute `{attr}`"),
                        span,
                    )
                    .label("not declared in this module's `import py` stub")
                    .note("the stub block lists the only names checked for this module")
                    .help("declare it in the stub block if the module really provides it")
                    .suggest_names(&suggestions),
                ));
            return;
        }

        if !known_fn || !all_positional {
            return;
        }
        let Some(arities) = self.py_fn_arity.get(module).and_then(|m| m.get(attr)) else {
            return;
        };
        if arities.is_empty() {
            return;
        }
        let accepts = arities
            .iter()
            .any(|(min, max)| arg_count >= *min && max.is_none_or(|m| arg_count <= m));
        if accepts {
            return;
        }
        let expected = describe_arity(arities);
        self.errors
            .push(crate::semantic::error::SemanticError::rich(
                crate::compile::errors::Diagnostic::error(
                    "E0602",
                    format!("`{display}.{attr}` takes {expected}, but {arg_count} were given"),
                    span,
                )
                .label(format!("called with {arg_count} argument(s) here"))
                .help("adjust the call to match the function's signature"),
            ));
    }

    pub(super) fn resolve_py_fn_overload(
        &self,
        module: &str,
        fn_name: &str,
        arg_tys: &[Type],
    ) -> Option<Type> {
        let fn_overloads = self
            .py_module_fns
            .get(module)
            .and_then(|m| m.get(fn_name))?;
        Some(Self::best_overload(fn_overloads, arg_tys)?.clone())
    }

    pub(super) fn resolve_py_field(&self, module: &str, class: &str, field: &str) -> Option<Type> {
        self.py_class_fields
            .get(&(module.to_string(), class.to_string()))
            .and_then(|m| m.get(field))
            .cloned()
    }

    pub(super) fn resolve_py_method_overload(
        &self,
        module: &str,
        class: &str,
        method: &str,
        arg_tys: &[Type],
    ) -> Option<Type> {
        let overloads = self
            .py_class_methods
            .get(&(module.to_string(), class.to_string()))
            .and_then(|m| m.get(method))?;
        Some(Self::best_overload(overloads, arg_tys)?.clone())
    }

    fn best_overload<'a>(overloads: &'a [(Vec<Type>, Type)], arg_tys: &[Type]) -> Option<&'a Type> {
        let arity = arg_tys.len();
        let candidates: Vec<_> = overloads
            .iter()
            .filter(|(params, _)| params.is_empty() || params.len() == arity)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        // Prefer exact (non-PyObject) param matches, which avoids PyObject fallback
        // overloads swamping TypeVar-expanded concrete overloads.
        if candidates.len() > 1 {
            let exact = candidates.iter().find(|(params, _)| {
                !params.is_empty()
                    && params
                        .iter()
                        .zip(arg_tys)
                        .all(|(p, a)| !matches!(p, Type::PyObject) && Self::py_types_compat(p, a))
            });
            if let Some((_, ret)) = exact {
                return Some(ret);
            }
            // Fallback: any compatible (including PyObject-param)
            let any = candidates.iter().find(|(params, _)| {
                !params.is_empty()
                    && params
                        .iter()
                        .zip(arg_tys)
                        .all(|(p, a)| Self::py_types_compat(p, a))
            });
            if let Some((_, ret)) = any {
                return Some(ret);
            }
        }
        candidates.first().map(|(_, ret)| ret)
    }

    fn py_types_compat(param: &Type, arg: &Type) -> bool {
        match (param, arg) {
            (Type::PyObject, _) | (_, Type::PyObject) => true,
            (Type::PyNamed(pm, pn), Type::PyNamed(am, an)) => pm == am && pn == an,
            (a, b) => a == b,
        }
    }

    pub(super) fn register_pyi(&mut self, alias: &str, info: PyiInfo) {
        for type_name in &info.types {
            let named = Type::PyNamed(alias.to_string(), type_name.clone());
            self.py_module_types
                .entry(alias.to_string())
                .or_default()
                .insert(type_name.clone(), named.clone());
            self.define_type(type_name, named, false);
        }
        for (raw_name, preferred) in &info.aliases {
            let named = Type::PyNamed(alias.to_string(), preferred.clone());
            self.py_module_types
                .entry(alias.to_string())
                .or_default()
                .insert(raw_name.clone(), named);
        }

        let snapshot = self.py_module_types.clone();
        for (fn_name, overloads) in info.fns {
            for sig in overloads {
                let ret_ty = Self::pyi_str_to_type(alias, &sig.ret, &info.aliases, &snapshot);
                let param_tys: Vec<Type> = sig
                    .params
                    .iter()
                    .map(|p| Self::pyi_str_to_type(alias, p, &info.aliases, &snapshot))
                    .collect();
                self.py_module_fns
                    .entry(alias.to_string())
                    .or_default()
                    .entry(fn_name.clone())
                    .or_default()
                    .push((param_tys.clone(), ret_ty.clone()));
                self.py_fn_arity
                    .entry(alias.to_string())
                    .or_default()
                    .entry(fn_name.clone())
                    .or_default()
                    .push((sig.min, sig.max));
                let mangled = format!("{}::{}", alias, fn_name);
                if self.lookup_type(&mangled).is_none() {
                    self.define_type(
                        &mangled,
                        Type::Fn(param_tys, Box::new(ret_ty), vec![]),
                        false,
                    );
                }
            }
        }

        for (cls_name, field_map) in info.fields {
            let key = (alias.to_string(), cls_name.clone());
            let typed_fields: HashMap<String, Type> = field_map
                .into_iter()
                .map(|(field_name, type_str)| {
                    let ty = Self::pyi_str_to_type(alias, &type_str, &info.aliases, &snapshot);
                    (field_name, ty)
                })
                .collect();
            self.py_class_fields.insert(key, typed_fields);
        }

        for (cls_name, method_map) in info.methods {
            let key = (alias.to_string(), cls_name.clone());
            let typed_methods: HashMap<String, Vec<(Vec<Type>, Type)>> = method_map
                .into_iter()
                .map(|(method_name, sigs)| {
                    let typed_sigs = sigs
                        .into_iter()
                        .map(|sig| {
                            let ret_ty =
                                Self::pyi_str_to_type(alias, &sig.ret, &info.aliases, &snapshot);
                            let param_tys: Vec<Type> = sig
                                .params
                                .iter()
                                .map(|p| Self::pyi_str_to_type(alias, p, &info.aliases, &snapshot))
                                .collect();
                            (param_tys, ret_ty)
                        })
                        .collect();
                    (method_name, typed_sigs)
                })
                .collect();
            self.py_class_methods.insert(key, typed_methods);
        }
    }

    fn pyi_str_to_type(
        alias: &str,
        name: &str,
        aliases: &HashMap<String, String>,
        type_map: &HashMap<String, HashMap<String, Type>>,
    ) -> Type {
        match name {
            // Introspected scalars are CPython objects at runtime, not native values,
            // so keep them dynamic; explicit stub blocks still bind native via resolve_type_expr.
            "float" | "int" | "bool" | "str" => Type::PyObject,
            "None" => Type::Null,
            "PyObject" => Type::PyObject,
            other => {
                let preferred = aliases.get(other).map(|s| s.as_str()).unwrap_or(other);
                if let Some(m) = type_map.get(alias)
                    && let Some(ty) = m.get(preferred)
                {
                    return ty.clone();
                }
                Type::PyNamed(alias.to_string(), preferred.to_string())
            }
        }
    }

    pub(super) fn is_mutable(&self, name: &str) -> bool {
        for scope in self.mut_env.iter().rev() {
            if let Some(is_mut) = scope.get(name) {
                return *is_mut;
            }
        }
        false
    }

    pub fn hoist_types(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            match &stmt.kind {
                crate::parser::StmtKind::Struct {
                    name, type_params, ..
                } => {
                    let abstract_args = type_params
                        .iter()
                        .map(|p| Type::Param(p.clone()))
                        .collect::<Vec<_>>();
                    self.define_type(
                        name,
                        Type::Struct(name.clone(), abstract_args, false),
                        false,
                    );
                }
                crate::parser::StmtKind::Enum {
                    name, type_params, ..
                } => {
                    let abstract_args = type_params
                        .iter()
                        .map(|p| Type::Param(p.clone()))
                        .collect::<Vec<_>>();
                    self.define_type(name, Type::Enum(name.clone(), abstract_args), false);
                }
                crate::parser::StmtKind::Trait {
                    name, type_params, ..
                } => {
                    let abstract_args = type_params
                        .iter()
                        .map(|p| Type::Param(p.clone()))
                        .collect::<Vec<_>>();
                    self.define_type(name, Type::TraitObject(name.clone(), abstract_args), false);
                }
                _ => {}
            }
        }
    }

    fn hoist_struct_fields(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if let crate::parser::StmtKind::Struct {
                name,
                type_params,
                fields,
                ..
            } = &stmt.kind
            {
                self.enter_scope();
                for tp in type_params {
                    self.define_type(tp, Type::Param(tp.clone()), false);
                }
                for field in fields {
                    let field_ty = field
                        .type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .unwrap_or(Type::Any);
                    self.field_types
                        .insert((name.clone(), field.name.clone()), field_ty);
                }
                self.leave_scope();
            }
        }
    }

    pub fn check_program(&mut self, program: &Program) {
        self.hoist_types(&program.stmts);
        self.hoist_struct_fields(&program.stmts);

        for stmt in &program.stmts {
            self.check_stmt(stmt);
        }

        let ids: Vec<usize> = self.expr_types.keys().cloned().collect();
        for id in ids {
            let ty = self.expr_types.get(&id).unwrap().clone();
            let final_ty = self.apply_subst_final(ty);
            self.expr_types.insert(id, final_ty);
        }

        for i in 0..self.type_env.len() {
            let names: Vec<String> = self.type_env[i].keys().cloned().collect();
            for name in names {
                let ty = self.type_env[i].get(&name).unwrap().clone();
                let final_ty = self.apply_subst_final(ty);
                self.type_env[i].insert(name, final_ty);
            }
        }
    }

    pub(super) fn check_block(&mut self, stmts: &[Stmt]) {
        self.enter_scope();
        for s in stmts {
            self.check_stmt(s);
        }
        self.leave_scope();
    }

    pub(super) fn instantiate(&mut self, ty: Type) -> Type {
        match ty {
            Type::Fn(params, ret, args) => {
                if args.is_empty() {
                    return Type::Fn(params, ret, args);
                }
                let mut subst = HashMap::default();
                let mut fresh_args = Vec::new();
                for arg in &args {
                    if let Type::Param(name) = arg {
                        let var = self.fresh_var();
                        subst.insert(name.clone(), var.clone());
                        fresh_args.push(var);
                    } else {
                        fresh_args.push(arg.clone());
                    }
                }

                let instantiated_params = params
                    .into_iter()
                    .map(|p| self.replace_params_with_vars(p, &subst))
                    .collect();
                let instantiated_ret = self.replace_params_with_vars(*ret, &subst);

                Type::Fn(instantiated_params, Box::new(instantiated_ret), fresh_args)
            }
            Type::Struct(name, args, is_ffi) => {
                let mut fresh_args = Vec::new();
                for arg in args {
                    if let Type::Param(_) = arg {
                        fresh_args.push(self.fresh_var());
                    } else {
                        fresh_args.push(arg);
                    }
                }
                Type::Struct(name, fresh_args, is_ffi)
            }
            Type::Enum(name, args) => {
                let mut fresh_args = Vec::new();
                for arg in args {
                    if let Type::Param(_) = arg {
                        fresh_args.push(self.fresh_var());
                    } else {
                        fresh_args.push(arg);
                    }
                }
                Type::Enum(name, fresh_args)
            }
            _ => ty,
        }
    }

    fn replace_params_with_vars(&self, ty: Type, subst: &HashMap<String, Type>) -> Type {
        match ty {
            Type::Param(name) => subst.get(&name).cloned().unwrap_or(Type::Param(name)),
            Type::List(inner) => Type::List(Box::new(self.replace_params_with_vars(*inner, subst))),
            Type::Set(inner) => Type::Set(Box::new(self.replace_params_with_vars(*inner, subst))),
            Type::Dict(k, v) => Type::Dict(
                Box::new(self.replace_params_with_vars(*k, subst)),
                Box::new(self.replace_params_with_vars(*v, subst)),
            ),
            Type::Tuple(elems) => Type::Tuple(
                elems
                    .into_iter()
                    .map(|e| self.replace_params_with_vars(e, subst))
                    .collect(),
            ),
            Type::Fn(params, ret, args) => Type::Fn(
                params
                    .into_iter()
                    .map(|p| self.replace_params_with_vars(p, subst))
                    .collect(),
                Box::new(self.replace_params_with_vars(*ret, subst)),
                args.into_iter()
                    .map(|a| self.replace_params_with_vars(a, subst))
                    .collect(),
            ),
            Type::Ref(inner) => Type::Ref(Box::new(self.replace_params_with_vars(*inner, subst))),
            Type::MutRef(inner) => {
                Type::MutRef(Box::new(self.replace_params_with_vars(*inner, subst)))
            }
            Type::Ptr(inner) => Type::Ptr(Box::new(self.replace_params_with_vars(*inner, subst))),
            Type::Future(inner) => {
                Type::Future(Box::new(self.replace_params_with_vars(*inner, subst)))
            }
            Type::Struct(name, args, is_ffi) => Type::Struct(
                name,
                args.into_iter()
                    .map(|a| self.replace_params_with_vars(a, subst))
                    .collect(),
                is_ffi,
            ),
            Type::Enum(name, args) => Type::Enum(
                name,
                args.into_iter()
                    .map(|a| self.replace_params_with_vars(a, subst))
                    .collect(),
            ),
            _ => ty,
        }
    }

    pub(super) fn get_struct_subst(
        &self,
        struct_name: &str,
        type_args: &[Type],
    ) -> HashMap<String, Type> {
        let mut subst = HashMap::default();
        if let Some(Type::Struct(_, params, _)) = self.lookup_type(struct_name) {
            for (p, a) in params.iter().zip(type_args) {
                if let Type::Param(name) = p {
                    subst.insert(name.clone(), a.clone());
                }
            }
        }
        subst
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semantic::Resolver;

    fn pipeline(src: &str) -> TypeChecker {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        tc
    }

    fn python3_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn py_import_missing_module_errors() {
        if !python3_available() {
            return;
        }
        let tc = pipeline("import py \"olive_definitely_missing_xyz\" as m\n");
        assert!(
            tc.errors
                .iter()
                .any(|e| format!("{e}").contains("cannot be imported")),
            "expected a module-not-found error, got: {:?}",
            tc.errors
        );
    }

    #[test]
    fn py_import_real_module_no_error() {
        if !python3_available() {
            return;
        }
        let tc = pipeline("import py \"json\" as j\n");
        assert!(
            tc.errors.is_empty(),
            "importable module without a stub must not error: {:?}",
            tc.errors
        );
    }

    #[test]
    fn match_error_variant_in_union_ok() {
        // `Error` is a built-in enum, so `case Error(e)` on `T | Error` resolves
        // instead of erroring with "expected Enum or Union type".
        let tc = pipeline(
            "fn f(x: i64 | Error):\n    match x:\n        case Error(e):\n            pass\n        case _:\n            pass\n",
        );
        assert!(
            tc.errors.is_empty(),
            "matching Error in a union must type-check: {:?}",
            tc.errors
        );
    }

    #[test]
    fn no_errors_on_valid_let() {
        let tc = pipeline("let x = 42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn no_errors_on_str_literal() {
        let tc = pipeline("let s = \"hello\"\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn no_errors_on_arithmetic() {
        let tc = pipeline("let x = 1 + 2 * 3\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn function_return_type_mismatch_reported() {
        let tc = pipeline("fn foo() -> i64:\n    return \"wrong type\"\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn valid_function_return_no_error() {
        let tc = pipeline("fn foo() -> i64:\n    return 42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn bool_result_from_comparison() {
        let tc = pipeline("let b = 1 < 2\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn struct_instantiation_ok() {
        let tc = pipeline("struct Point:\n    x: i64\n    y: i64\n\nlet p = Point(1, 2)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn struct_field_access_ok() {
        let tc =
            pipeline("struct Point:\n    x: i64\n    y: i64\n\nlet p = Point(3, 4)\nlet v = p.x\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn generic_function_monomorphizes() {
        let tc = pipeline("fn identity[T](x: T) -> T:\n    return x\n\nlet y = identity(10)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn ffi_call_outside_unsafe_reported() {
        let tc = pipeline(
            "import \"/usr/lib/libc.so.6\" as libc:\n    fn getpid() -> i64\n\nlibc::getpid()\n",
        );
        assert!(
            tc.errors
                .iter()
                .any(|e| { e.to_diagnostic().headline().contains("unsafe") })
        );
    }

    #[test]
    fn ffi_call_inside_unsafe_ok() {
        let tc = pipeline(
            "import \"/usr/lib/libc.so.6\" as libc:\n    fn getpid() -> i64\n\nunsafe:\n    libc::getpid()\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn ffi_safe_decorator_no_unsafe_required() {
        let tc = pipeline(
            "import \"/usr/lib/libc.so.6\" as libc:\n    @safe\n    fn getpid() -> i64\n\nlibc::getpid()\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn nested_function_calls_ok() {
        let tc = pipeline(
            "fn add(a: i64, b: i64) -> i64:\n    return a + b\n\nlet r = add(add(1, 2), 3)\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn if_else_expression_ok() {
        let tc = pipeline("let x = 5\nif x > 3:\n    let y = 1\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn wrong_arg_count_reported() {
        let tc = pipeline("fn f(a: i64, b: i64) -> i64:\n    return a + b\n\nf(1)\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn recursive_function_ok() {
        let tc = pipeline(
            "fn fact(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn enum_variant_construction_ok() {
        let tc =
            pipeline("enum Shape:\n    Circle(i64)\n    Rect(i64, i64)\n\nlet c = Circle(5)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn union_type_ok() {
        let tc = pipeline("fn f(x: i64 | str) -> i64:\n    return 0\n\nf(42)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn list_homogeneity_inferred() {
        let tc = pipeline("let xs = [1, 2, 3]\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn dict_type_inferred() {
        let tc = pipeline("let d = {\"a\": 1, \"b\": 2}\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn tuple_type_inferred() {
        let tc = pipeline("let t = (1, \"hello\", True)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn while_loop_ok() {
        let tc = pipeline("let mut i = 0\nwhile i < 10:\n    i = i + 1\n");
        assert!(tc.errors.is_empty(), "errors: {:?}", tc.errors);
    }

    #[test]
    fn for_loop_ok() {
        let tc = pipeline("for x in [1, 2, 3]:\n    let y = x + 1\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn method_call_on_struct_ok() {
        let tc = pipeline(
            "struct Counter:\n    val: i64\n\nimpl Counter:\n    fn inc(self) -> i64:\n        return self.val + 1\n\nlet c = Counter(0)\nlet v = c.inc()\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn generic_struct_ok() {
        let tc = pipeline(
            "struct Pair[A, B]:\n    first: A\n    second: B\n\nlet p = Pair(1, \"two\")\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn return_type_mismatch_in_branch_reported() {
        let tc = pipeline(
            "fn f(x: i64) -> i64:\n    if x > 0:\n        return \"wrong\"\n    return 0\n",
        );
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn mutable_let_reassignment_ok() {
        let tc = pipeline("let mut x = 0\nx = 42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn immutable_reassignment_reported() {
        let tc = pipeline("let x = 0\nx = 42\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn const_declaration_ok() {
        let tc = pipeline("const PI = 3\nlet r = PI * 2\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn ptr_type_annotation_ok() {
        let tc = pipeline("fn deref(p: *i64) -> i64:\n    return 0\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn ptr_ptr_type_ok() {
        let tc = pipeline("fn f(p: *(*i64)) -> i64:\n    return 0\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn match_exhaustive_ok() {
        let tc = pipeline(
            "enum Color:\n    Red(i64)\n    Green(i64)\n    Blue(i64)\n\nlet c = Red(0)\nmatch c:\n    case Red(v):\n        let x = v\n    case _:\n        let x = 0\n",
        );
        assert!(tc.errors.is_empty(), "errors: {:?}", tc.errors);
    }

    #[test]
    fn trait_method_defined_ok() {
        let tc = pipeline(
            "trait Printable:\n    fn display(self) -> str:\n        return \"\"\n\nstruct Pt:\n    x: i64\n\nimpl Printable for Pt:\n    fn display(self) -> str:\n        return str(self.x)\n",
        );
        assert!(tc.errors.is_empty(), "errors: {:?}", tc.errors);
    }

    #[test]
    fn trait_signature_mismatch_reported() {
        let tc = pipeline(
            "trait Drawable:\n    fn draw(self) -> i64:\n        return 0\n\nstruct Circle:\n    radius: i64\n\nimpl Drawable for Circle:\n    fn draw(self) -> str:\n        return \"wrong\"\n",
        );
        assert!(
            !tc.errors.is_empty(),
            "Expected error for trait signature mismatch"
        );
    }

    #[test]
    fn nested_generics_ok() {
        let tc = pipeline("fn wrap[T](x: T) -> [T]:\n    return [x]\n\nlet r = wrap(42)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn float_arithmetic_ok() {
        let tc = pipeline("let x = 1.5 + 2.5\nlet y = x * 2.0\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn bool_operations_ok() {
        let tc = pipeline("let a = True\nlet b = False\nlet c = a and b\nlet d = a or b\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn string_concat_ok() {
        let tc = pipeline("let s = \"hello\" + \" world\"\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn list_indexing_ok() {
        let tc = pipeline("let xs = [10, 20, 30]\nlet v = xs[1]\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn nested_struct_field_access_ok() {
        let tc = pipeline(
            "struct Inner:\n    v: i64\nstruct Outer:\n    inner: Inner\nlet o = Outer(Inner(5))\nlet v = o.inner.v\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn higher_order_function_ok() {
        let tc = pipeline("fn apply(f: fn(i64) -> i64, x: i64) -> i64:\n    return f(x)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn introspected_module_unknown_attr_not_flagged() {
        // A `.pyi` surface is incomplete, so an absent name must not error.
        let mut tc = TypeChecker::new();
        tc.py_module_fns
            .entry("pg".into())
            .or_default()
            .insert("init".into(), vec![(vec![], Type::Null)]);
        tc.check_py_call("pg", "Surface", 2, true, crate::span::Span::default());
        assert!(tc.errors.is_empty(), "introspected module must not error");
    }

    #[test]
    fn explicit_stub_unknown_attr_flagged() {
        let mut tc = TypeChecker::new();
        tc.py_module_fns
            .entry("m".into())
            .or_default()
            .insert("sqrt".into(), vec![(vec![Type::Float], Type::Float)]);
        tc.py_explicit_modules.insert("m".into());
        tc.check_py_call("m", "cbrt", 1, true, crate::span::Span::default());
        assert!(
            tc.errors
                .iter()
                .any(|e| e.to_diagnostic().code() == Some("E0601"))
        );
    }

    #[test]
    fn unknown_attr_names_real_module_not_alias() {
        let mut tc = TypeChecker::new();
        tc.py_module_fns
            .entry("m".into())
            .or_default()
            .insert("sqrt".into(), vec![(vec![Type::Float], Type::Float)]);
        tc.py_explicit_modules.insert("m".into());
        tc.py_alias_module.insert("m".into(), "math".into());
        tc.check_py_call("m", "cbrt", 1, true, crate::span::Span::default());
        let diag = tc
            .errors
            .iter()
            .map(|e| e.to_diagnostic())
            .find(|d| d.code() == Some("E0601"))
            .expect("E0601 expected");
        assert!(
            diag.headline().contains("`math`"),
            "diagnostic should name the real module, got: {}",
            diag.headline()
        );
        assert!(!diag.headline().contains("`m`"));
    }

    #[test]
    fn var_counter_isolated_between_instances() {
        let tc1 = pipeline("let x = [1]\n");
        let tc2 = pipeline("let y = [2]\n");
        assert!(tc1.errors.is_empty());
        assert!(tc2.errors.is_empty());
        assert_eq!(
            tc1.var_counter, tc2.var_counter,
            "each TypeChecker instance must use its own counter"
        );
    }
}
