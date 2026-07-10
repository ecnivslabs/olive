use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    Variable,
    Function,
    Struct,
    Enum,
    TypeAlias,
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
    /// Set once the name is referenced, so a binding that is never read can be
    /// reported when its scope closes. Defaults to false; bindings the unused
    /// lint should ignore (type parameters, say) are marked used on definition.
    pub used: bool,
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

    /// Marks the innermost binding of `name` as referenced, so it is not later
    /// reported as unused. Shadowed outer bindings are left untouched, exactly
    /// as a read resolves to the innermost one.
    pub fn mark_used(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(sym) = scope.symbols.get_mut(name) {
                sym.used = true;
                return;
            }
        }
    }

    /// Marks every name in `names` used. Used for bindings whose absence of a
    /// read is not a mistake worth reporting (type parameters).
    pub fn mark_used_in_current(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut()
            && let Some(sym) = scope.symbols.get_mut(name)
        {
            sym.used = true;
        }
    }

    /// Pops the innermost scope and returns the bindings within it that were
    /// never referenced, so the caller can warn about them. The global scope is
    /// never popped, so module-level bindings are not reported here.
    pub fn pop_unused(&mut self) -> Vec<Symbol> {
        if self.scopes.len() <= 1 {
            return Vec::new();
        }
        let scope = self.scopes.pop().unwrap();
        let reportable = matches!(scope.kind, ScopeKind::Function | ScopeKind::Block);
        if !reportable {
            return Vec::new();
        }
        scope
            .symbols
            .into_values()
            .filter(|s| {
                !s.used
                    && matches!(
                        s.kind,
                        SymbolKind::Variable | SymbolKind::Parameter | SymbolKind::LoopVar
                    )
                    && !s.name.starts_with('_')
                    && s.name != "self"
            })
            .collect()
    }

    /// Every name currently visible, innermost scope first. Used to surface
    /// `did you mean` suggestions for unresolved identifiers.
    pub fn visible_names(&self) -> impl Iterator<Item = &str> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.symbols.keys().map(String::as_str))
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
            used: false,
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
        st.pop_unused();
        assert_eq!(st.scopes.len(), 1);
    }

    #[test]
    fn pop_does_not_remove_global() {
        let mut st = SymbolTable::new();
        st.pop_unused();
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
        st.pop_unused();
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
            used: false,
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
