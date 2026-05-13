mod expr;
mod patterns;
mod stmt;
mod unify;

use super::error::SemanticError;
use super::types::Type;
use crate::parser::{Program, Stmt};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub struct TypeChecker {
    pub(super) substitutions: HashMap<usize, Type>,
    pub expr_types: HashMap<usize, Type>,
    pub type_env: Vec<HashMap<String, Type>>,
    pub(super) current_return_type: Option<Type>,
    pub errors: Vec<SemanticError>,
    pub(super) mut_env: Vec<HashMap<String, bool>>,
    pub field_types: HashMap<(String, String), Type>,
    pub enum_variants: HashMap<String, Vec<String>>,
    pub(super) current_struct: Option<String>,
    pub(super) async_depth: usize,
    pub(super) vararg_fns: HashSet<String>,
    pub(super) traits: HashMap<String, Vec<String>>,
    pub(super) type_traits: HashSet<(String, String)>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut global_env = HashMap::default();

        let builtins = [
            ("print", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("str", Type::Fn(vec![Type::Any], Box::new(Type::Str))),
            ("int", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("i64", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("i32", Type::Fn(vec![Type::Any], Box::new(Type::I32))),
            ("i16", Type::Fn(vec![Type::Any], Box::new(Type::I16))),
            ("i8", Type::Fn(vec![Type::Any], Box::new(Type::I8))),
            ("u64", Type::Fn(vec![Type::Any], Box::new(Type::U64))),
            ("u32", Type::Fn(vec![Type::Any], Box::new(Type::U32))),
            ("u16", Type::Fn(vec![Type::Any], Box::new(Type::U16))),
            ("u8", Type::Fn(vec![Type::Any], Box::new(Type::U8))),
            ("float", Type::Fn(vec![Type::Any], Box::new(Type::Float))),
            ("f64", Type::Fn(vec![Type::Any], Box::new(Type::Float))),
            ("f32", Type::Fn(vec![Type::Any], Box::new(Type::F32))),
            ("bool", Type::Fn(vec![Type::Any], Box::new(Type::Bool))),
            ("type", Type::Fn(vec![Type::Any], Box::new(Type::Str))),
            ("len", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            (
                "list_new",
                Type::Fn(vec![Type::Int], Box::new(Type::List(Box::new(Type::Any)))),
            ),
            (
                "__olive_async_file_read",
                Type::Fn(vec![Type::Str], Box::new(Type::Future(Box::new(Type::Str)))),
            ),
            (
                "__olive_async_file_write",
                Type::Fn(
                    vec![Type::Str, Type::Str],
                    Box::new(Type::Future(Box::new(Type::Int))),
                ),
            ),
            (
                "__olive_gather",
                Type::Fn(vec![Type::Any], Box::new(Type::List(Box::new(Type::Any)))),
            ),
            (
                "__olive_free_future",
                Type::Fn(vec![Type::Any], Box::new(Type::Int)),
            ),
            (
                "__olive_math_sin",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_cos",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_tan",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_asin",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_acos",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_atan",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_atan2",
                Type::Fn(vec![Type::Float, Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_log",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_log10",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_exp",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_random_seed",
                Type::Fn(vec![Type::Int], Box::new(Type::Null)),
            ),
            (
                "__olive_random_get",
                Type::Fn(vec![], Box::new(Type::Float)),
            ),
            (
                "__olive_random_int",
                Type::Fn(vec![Type::Int, Type::Int], Box::new(Type::Int)),
            ),
            (
                "__olive_net_tcp_connect",
                Type::Fn(vec![Type::Str], Box::new(Type::Int)),
            ),
            (
                "__olive_net_tcp_send",
                Type::Fn(vec![Type::Int, Type::Str], Box::new(Type::Int)),
            ),
            (
                "__olive_net_tcp_recv",
                Type::Fn(vec![Type::Int, Type::Int], Box::new(Type::Str)),
            ),
            (
                "__olive_net_tcp_close",
                Type::Fn(vec![Type::Int], Box::new(Type::Null)),
            ),
            (
                "__olive_http_get",
                Type::Fn(vec![Type::Str], Box::new(Type::Str)),
            ),
            (
                "__olive_http_post",
                Type::Fn(vec![Type::Str, Type::Str], Box::new(Type::Str)),
            ),
            (
                "__olive_spawn_task",
                Type::Fn(vec![Type::Any], Box::new(Type::Future(Box::new(Type::Any)))),
            ),
        ];

        for (name, ty) in builtins {
            global_env.insert(name.to_string(), ty);
        }

        Self {
            substitutions: HashMap::default(),
            expr_types: HashMap::default(),
            type_env: vec![global_env],
            current_return_type: None,
            errors: Vec::new(),
            mut_env: vec![HashMap::default()],
            field_types: HashMap::default(),
            enum_variants: HashMap::default(),
            current_struct: None,
            async_depth: 0,
            vararg_fns: HashSet::default(),
            traits: HashMap::default(),
            type_traits: HashSet::default(),
        }
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

    pub(super) fn is_mutable(&self, name: &str) -> bool {
        for scope in self.mut_env.iter().rev() {
            if let Some(is_mut) = scope.get(name) {
                return *is_mut;
            }
        }
        false
    }

    pub fn check_program(&mut self, program: &Program) {
        for stmt in &program.stmts {
            self.check_stmt(stmt);
        }

        let ids: Vec<usize> = self.expr_types.keys().cloned().collect();
        for id in ids {
            let ty = self.expr_types.get(&id).unwrap().clone();
            let final_ty = self.apply_subst(ty);
            self.expr_types.insert(id, final_ty);
        }
    }

    pub(super) fn check_block(&mut self, stmts: &[Stmt]) {
        self.enter_scope();
        for s in stmts {
            self.check_stmt(s);
        }
        self.leave_scope();
    }
}
