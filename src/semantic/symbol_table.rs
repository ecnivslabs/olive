use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    Variable,
    Function,
    Struct,
    Enum,
    Parameter,
    LoopVar,
    Import,
    NativeImport,
    PyImport,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub is_private: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScopeKind {
    Global,
    Function,
    Struct,
    Block,
    Comprehension,
}

pub struct Scope {
    #[cfg_attr(not(test), allow(dead_code))]
    pub kind: ScopeKind,
    symbols: HashMap<String, Symbol>,
}

impl Scope {
    pub fn new(kind: ScopeKind) -> Self {
        Self {
            kind,
            symbols: HashMap::default(),
        }
    }

    pub fn define(&mut self, sym: Symbol) -> Option<Symbol> {
        self.symbols.insert(sym.name.clone(), sym)
    }

    pub fn get(&self, name: &str) -> Option<&Symbol> {
        self.symbols.get(name)
    }
}

pub struct SymbolTable {
    scopes: Vec<Scope>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::new(ScopeKind::Global)],
        }
    }

    pub fn push(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope::new(kind));
    }

    pub fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    pub fn define(&mut self, sym: Symbol) -> Option<Symbol> {
        self.scopes.last_mut().unwrap().define(sym)
    }

    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for scope in self.scopes.iter().rev() {
            if let Some(sym) = scope.get(name) {
                return Some(sym);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn sym(name: &str, kind: SymbolKind) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind,
            span: Span {
                file_id: 0,
                line: 1,
                col: 1,
                start: 0,
                end: 1,
            },
            is_private: false,
        }
    }

    #[test]
    fn new_has_global_scope() {
        let st = SymbolTable::new();
        assert_eq!(st.scopes.len(), 1);
        assert_eq!(st.scopes[0].kind, ScopeKind::Global);
    }

    #[test]
    fn define_and_lookup_variable() {
        let mut st = SymbolTable::new();
        st.define(sym("x", SymbolKind::Variable));
        let found = st.lookup("x");
        assert!(found.is_some());
        assert_eq!(found.unwrap().kind, SymbolKind::Variable);
        assert_eq!(found.unwrap().name, "x");
    }

    #[test]
    fn define_and_lookup_function() {
        let mut st = SymbolTable::new();
        st.define(sym("foo", SymbolKind::Function));
        let found = st.lookup("foo");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "foo");
    }

    #[test]
    fn lookup_missing_returns_none() {
        let st = SymbolTable::new();
        assert!(st.lookup("nonexistent").is_none());
    }

    #[test]
    fn push_adds_scope() {
        let mut st = SymbolTable::new();
        st.push(ScopeKind::Block);
        assert_eq!(st.scopes.len(), 2);
        assert_eq!(st.scopes[1].kind, ScopeKind::Block);
    }

    #[test]
    fn pop_removes_scope() {
        let mut st = SymbolTable::new();
        st.push(ScopeKind::Block);
        st.pop();
        assert_eq!(st.scopes.len(), 1);
    }

    #[test]
    fn pop_does_not_remove_global() {
        let mut st = SymbolTable::new();
        st.pop();
        assert_eq!(st.scopes.len(), 1);
    }

    #[test]
    fn inner_scope_shadows_outer() {
        let mut st = SymbolTable::new();
        st.define(sym("x", SymbolKind::Variable));
        st.push(ScopeKind::Block);
        st.define(sym("x", SymbolKind::Parameter));
        assert_eq!(st.lookup("x").unwrap().kind, SymbolKind::Parameter);
    }

    #[test]
    fn outer_visible_from_inner() {
        let mut st = SymbolTable::new();
        st.define(sym("outer", SymbolKind::Variable));
        st.push(ScopeKind::Block);
        assert!(st.lookup("outer").is_some());
    }

    #[test]
    fn inner_not_visible_from_outer() {
        let mut st = SymbolTable::new();
        st.push(ScopeKind::Function);
        st.define(sym("inner", SymbolKind::Variable));
        st.pop();
        assert!(st.lookup("inner").is_none());
    }

    #[test]
    fn scope_new() {
        let scope = Scope::new(ScopeKind::Function);
        assert_eq!(scope.kind, ScopeKind::Function);
    }

    #[test]
    fn scope_define_returns_previous() {
        let mut scope = Scope::new(ScopeKind::Block);
        assert!(scope.define(sym("x", SymbolKind::Variable)).is_none());
        let prev = scope.define(sym("x", SymbolKind::Parameter));
        assert!(prev.is_some());
        assert_eq!(prev.unwrap().kind, SymbolKind::Variable);
    }

    #[test]
    fn symbol_kind_partial_eq() {
        assert_eq!(SymbolKind::Variable, SymbolKind::Variable);
        assert_ne!(SymbolKind::Variable, SymbolKind::Function);
        assert_eq!(SymbolKind::LoopVar, SymbolKind::LoopVar);
    }

    #[test]
    fn private_symbol_flag() {
        let s = Symbol {
            name: "priv".into(),
            kind: SymbolKind::Variable,
            span: Span {
                file_id: 0,
                line: 1,
                col: 1,
                start: 0,
                end: 1,
            },
            is_private: true,
        };
        assert!(s.is_private);
    }

    #[test]
    fn define_overwrites_in_same_scope() {
        let mut st = SymbolTable::new();
        st.define(sym("x", SymbolKind::Variable));
        st.define(sym("x", SymbolKind::Function));
        assert_eq!(st.lookup("x").unwrap().kind, SymbolKind::Function);
    }
}
