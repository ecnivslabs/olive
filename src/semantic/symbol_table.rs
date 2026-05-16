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
    #[allow(dead_code)]
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
