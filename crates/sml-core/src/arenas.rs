use super::*;
// use sml_util::arena::Arena;
use std::cell::Cell;
use typed_arena::Arena;

pub struct OwnedCoreArena<'arena> {
    types: Arena<Type<'arena>>,
    vars: Arena<TypeVar<'arena>>,
    exprs: Arena<ExprKind<'arena>>,
    pats: Arena<PatKind<'arena>>,
}

impl<'arena> OwnedCoreArena<'arena> {
    pub fn new() -> OwnedCoreArena<'arena> {
        OwnedCoreArena {
            types: Arena::with_capacity(4096),
            vars: Arena::with_capacity(4096),
            exprs: Arena::with_capacity(4096),
            pats: Arena::with_capacity(4096),
        }
    }
    pub fn borrow(&'arena self) -> CoreArena<'arena> {
        CoreArena {
            types: TypeArena::new(&self.types, &self.vars),
            exprs: ExprArena::new(&self.exprs),
            pats: PatArena::new(&self.pats),
        }
    }
}

pub struct CoreArena<'ar> {
    pub types: TypeArena<'ar>,
    pub exprs: ExprArena<'ar>,
    pub pats: PatArena<'ar>,
}

pub struct ExprArena<'ar> {
    arena: &'ar Arena<ExprKind<'ar>>,
    fresh: Cell<u32>,
}

impl<'ar> ExprArena<'ar> {
    pub fn new(arena: &'ar Arena<ExprKind<'ar>>) -> ExprArena<'ar> {
        ExprArena {
            arena,
            fresh: Cell::new(0),
        }
    }

    pub fn alloc(&self, pk: ExprKind<'ar>) -> &'ar ExprKind<'ar> {
        self.arena.alloc(pk)
    }

    pub fn fresh_var(&self) -> &'ar ExprKind<'ar> {
        self.arena.alloc(ExprKind::Var(self.allocate_id()))
    }

    pub fn allocate_id(&self) -> Symbol {
        let x = self.fresh.get();
        self.fresh.set(x + 1);
        Symbol::Gensym(x)
    }

    pub fn tuple<I: IntoIterator<Item = Expr<'ar>>>(&self, iter: I) -> &'ar ExprKind<'ar> {
        let rows = iter
            .into_iter()
            .enumerate()
            .map(|(idx, ty)| Row {
                label: Symbol::tuple_field(idx as u32 + 1),
                data: ty,
                span: Span::dummy(),
            })
            .collect();

        self.alloc(ExprKind::Record(rows))
    }
}

pub struct PatArena<'ar> {
    arena: &'ar Arena<PatKind<'ar>>,
    _wild: &'ar PatKind<'ar>,
}

impl<'ar> PatArena<'ar> {
    pub fn new(arena: &'ar Arena<PatKind<'ar>>) -> PatArena<'ar> {
        PatArena {
            arena,
            _wild: arena.alloc(PatKind::Wild),
        }
    }

    pub fn alloc(&self, pk: PatKind<'ar>) -> &'ar PatKind<'ar> {
        self.arena.alloc(pk)
    }

    pub fn wild(&self) -> &'ar PatKind<'ar> {
        self._wild
    }

    pub fn tuple<I: IntoIterator<Item = Pat<'ar>>>(&self, iter: I) -> &'ar PatKind<'ar> {
        let rows = iter
            .into_iter()
            .enumerate()
            .map(|(idx, ty)| Row {
                label: Symbol::tuple_field(idx as u32 + 1),
                data: ty,
                span: Span::dummy(),
            })
            .collect();

        self.alloc(PatKind::Record(rows))
    }
}

pub struct TypeArena<'ar> {
    types: &'ar Arena<Type<'ar>>,
    vars: &'ar Arena<TypeVar<'ar>>,
    fresh: Cell<usize>,

    // We cache the builtin nullary type constructors
    _exn: &'ar Type<'ar>,
    _bool: &'ar Type<'ar>,
    _int: &'ar Type<'ar>,
    _str: &'ar Type<'ar>,
    _char: &'ar Type<'ar>,
    _unit: &'ar Type<'ar>,
}

impl<'ar> TypeArena<'ar> {
    pub fn new(types: &'ar Arena<Type<'ar>>, vars: &'ar Arena<TypeVar<'ar>>) -> TypeArena<'ar> {
        let _exn = types.alloc(Type::Con(builtin::tycons::T_EXN, Vec::new()));
        let _bool = types.alloc(Type::Con(builtin::tycons::T_BOOL, Vec::new()));
        let _int = types.alloc(Type::Con(builtin::tycons::T_INT, Vec::new()));
        let _str = types.alloc(Type::Con(builtin::tycons::T_STRING, Vec::new()));
        let _char = types.alloc(Type::Con(builtin::tycons::T_CHAR, Vec::new()));
        let _unit = types.alloc(Type::Con(builtin::tycons::T_UNIT, Vec::new()));

        TypeArena {
            types,
            vars,
            fresh: Cell::new(0),
            _exn,
            _bool,
            _int,
            _str,
            _char,
            _unit,
        }
    }

    pub fn alloc(&self, ty: Type<'ar>) -> &'ar Type<'ar> {
        self.types.alloc(ty)
    }

    pub fn fresh_var(&self) -> &'ar Type<'ar> {
        let tvar = self.fresh_type_var();
        self.types.alloc(Type::Var(tvar))
    }

    pub fn fresh_type_var(&self) -> &'ar TypeVar<'ar> {
        let x = self.fresh.get();
        self.fresh.set(x + 1);
        self.vars.alloc(TypeVar::new(x))
    }

    pub fn alloc_tuple<I: IntoIterator<Item = Type<'ar>>>(&self, iter: I) -> &'ar Type<'ar> {
        let rows = iter
            .into_iter()
            .enumerate()
            .map(|(idx, ty)| Row {
                label: Symbol::tuple_field(idx as u32 + 1),
                data: self.alloc(ty),
                span: Span::dummy(),
            })
            .collect();

        self.alloc(Type::Record(rows))
    }

    pub fn tuple<I: IntoIterator<Item = &'ar Type<'ar>>>(&self, iter: I) -> &'ar Type<'ar> {
        let rows = iter
            .into_iter()
            .enumerate()
            .map(|(idx, ty)| Row {
                label: Symbol::tuple_field(idx as u32 + 1),
                data: ty,
                span: Span::dummy(),
            })
            .collect();

        self.alloc(Type::Record(rows))
    }

    pub fn exn(&self) -> &'ar Type<'ar> {
        self._exn
    }

    pub fn unit(&self) -> &'ar Type<'ar> {
        self._unit
    }

    pub fn char(&self) -> &'ar Type<'ar> {
        self._char
    }

    pub fn int(&self) -> &'ar Type<'ar> {
        self._int
    }

    pub fn bool(&self) -> &'ar Type<'ar> {
        self._bool
    }

    pub fn string(&self) -> &'ar Type<'ar> {
        self._str
    }

    pub fn reff(&self, ty: &'ar Type<'ar>) -> &'ar Type<'ar> {
        self.types
            .alloc(Type::Con(builtin::tycons::T_REF, vec![ty]))
    }

    pub fn list(&self, ty: &'ar Type<'ar>) -> &'ar Type<'ar> {
        self.types
            .alloc(Type::Con(builtin::tycons::T_LIST, vec![ty]))
    }

    pub fn arrow(&self, dom: &'ar Type<'ar>, rng: &'ar Type<'ar>) -> &'ar Type<'ar> {
        self.types
            .alloc(Type::Con(builtin::tycons::T_ARROW, vec![dom, rng]))
    }
}
