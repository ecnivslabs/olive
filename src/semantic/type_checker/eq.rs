use super::TypeChecker;
use crate::semantic::types::Type;
use rustc_hash::FxHashSet as HashSet;

impl TypeChecker {
    /// Whether `ty` derives structural `==`/`!=` and hashing: `Fn` and
    /// `Future` don't (no defined equality), everything else does,
    /// recursing into struct fields / enum payloads / collection elements.
    /// `visiting` guards a self-referential struct/enum definition
    /// (`struct Node: next: Node | None`); an already-visiting name is
    /// assumed to support `==` rather than looping forever.
    pub(super) fn type_supports_eq(&self, ty: &Type) -> bool {
        self.type_supports_eq_visiting(ty, &mut HashSet::default())
    }

    fn type_supports_eq_visiting(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Type::Fn(..) | Type::Future(_) => false,
            Type::Struct(name, ..) => {
                if !visiting.insert(name.clone()) {
                    return true;
                }
                let supports = self.struct_fields.get(name).is_none_or(|fields| {
                    fields.iter().all(|f| {
                        self.field_types
                            .get(&(name.clone(), f.clone()))
                            .is_none_or(|t| self.type_supports_eq_visiting(t, visiting))
                    })
                });
                visiting.remove(name);
                supports
            }
            Type::Enum(name, _) => {
                if !visiting.insert(name.clone()) {
                    return true;
                }
                let supports = self.enum_defs.get(name).is_none_or(|variants| {
                    variants.iter().all(|(_, payload)| {
                        payload
                            .iter()
                            .all(|t| self.type_supports_eq_visiting(t, visiting))
                    })
                });
                visiting.remove(name);
                supports
            }
            Type::Tuple(elems) => elems
                .iter()
                .all(|t| self.type_supports_eq_visiting(t, visiting)),
            Type::List(e) | Type::Set(e) | Type::Ref(e) | Type::MutRef(e) => {
                self.type_supports_eq_visiting(e, visiting)
            }
            Type::Dict(k, v) => {
                self.type_supports_eq_visiting(k, visiting)
                    && self.type_supports_eq_visiting(v, visiting)
            }
            Type::Union(members) => members
                .iter()
                .all(|t| self.type_supports_eq_visiting(t, visiting)),
            _ => true,
        }
    }
}
