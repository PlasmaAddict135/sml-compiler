mod exprs;
mod pats;
mod prec;
mod stack;
mod types;

use super::builtin::constructors::*;
use super::builtin::tycons::*;
use super::inference::Constraint;
use super::*;
use sml_frontend::ast;
use sml_frontend::parser::precedence::{self, Fixity, Precedence, Query};
use sml_util::diagnostics::Diagnostic;
use sml_util::interner::{S_CONS, S_FALSE, S_NIL, S_REF, S_TRUE};
use stack::Stack;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

/// Identifier status for the Value Environment, as defined in the Defn.
#[derive(Copy, Clone, Debug)]
pub enum IdStatus {
    Exn(Constructor),
    Con(Constructor),
    Var,
}

/// Essentially a minimized Value Environment (VE) containing only datatype
/// constructors, and without the indirection of going from names->id->def
#[derive(Clone, Debug)]
pub struct Cons {
    name: Symbol,
    scheme: Scheme,
}

/// TyStr, a [`TypeStructure`] from the Defn. This is a component of the
/// Type Environment, TE
#[derive(Clone, Debug)]
pub enum TypeStructure {
    /// TyStr (t, VE), a datatype
    Datatype(Tycon, Vec<Cons>),
    /// TyStr (_, VE), a definition. Rather than include a whole VE hashmap,
    /// we can include just a single entry
    Scheme(Scheme),
    /// TyStr (t, {}), a name
    Tycon(Tycon),
}

impl TypeStructure {
    pub fn arity(&self) -> usize {
        match self {
            TypeStructure::Tycon(con) | TypeStructure::Datatype(con, _) => con.arity,
            TypeStructure::Scheme(s) => s.arity(),
        }
    }

    pub fn apply(&self, args: Vec<Type>) -> Type {
        match self {
            TypeStructure::Tycon(con) | TypeStructure::Datatype(con, _) => Type::Con(*con, args),
            TypeStructure::Scheme(s) => s.apply(args),
        }
    }
}
/// An environment scope, that can hold a collection of type and expr bindings
#[derive(Default, Debug)]
pub struct Namespace {
    parent: Option<usize>,
    types: HashMap<Symbol, TypeId>,
    values: HashMap<Symbol, ExprId>,
    infix: HashMap<Symbol, Fixity>,
}

#[derive(Default, Debug)]
pub struct Context {
    // stacks for alpha-renaming
    tyvars: Stack<TypeVar>,

    namespaces: Vec<Namespace>,
    current: usize,

    // All types live here, indexed by `TypeId`
    types: Vec<TypeStructure>,
    // All values live here, indexed by `ExprId`
    values: Vec<(Scheme, IdStatus)>,

    // Generation of fresh type variables
    tyvar_id: Cell<usize>,
    tyvar_rank: usize,

    // Generation of fresh value variables
    var_id: Cell<u32>,

    // Exported top-level decls saved here
    decls: Vec<Decl>,
}

impl Namespace {
    pub fn with_parent(id: usize) -> Namespace {
        Namespace {
            parent: Some(id),
            types: HashMap::with_capacity(32),
            values: HashMap::with_capacity(64),
            infix: HashMap::with_capacity(16),
        }
    }
}

impl Context {
    pub fn new() -> Context {
        let mut ctx = Context::default();
        ctx.namespaces.push(Namespace::default());

        for tc in &crate::builtin::tycons::T_BUILTINS {
            ctx.define_type(tc.name, TypeStructure::Tycon(*tc));
        }

        ctx.define_value(
            S_NIL,
            Scheme::Poly(
                vec![0],
                Type::Con(builtin::tycons::T_LIST, vec![Type::Var(TypeVar::new(0, 0))]),
            ),
            IdStatus::Con(builtin::constructors::C_NIL),
        );
        // ctx.define_value(S_CONS, Scheme::Poly(vec![0], Type::Con(builtin::tycons::T_LIST, vec![Type::Var(TypeVar::new(0, 0))])), IdStatus::Con(builtin::constructors::C_CONS));
        ctx.define_value(
            S_TRUE,
            Scheme::Mono(Type::bool()),
            IdStatus::Con(builtin::constructors::C_TRUE),
        );
        ctx.define_value(
            S_FALSE,
            Scheme::Mono(Type::bool()),
            IdStatus::Con(builtin::constructors::C_FALSE),
        );
        ctx.elab_decl_fixity(&ast::Fixity::Infixr, 4, S_CONS)
            .unwrap();
        ctx.tyvar_id.set(1);
        ctx
    }
    /// Keep track of the type variable stack, while executing the combinator
    /// function `f` on `self`. Any stack growth is popped off after `f`
    /// returns.
    fn with_tyvars<T, F: FnMut(&mut Context) -> T>(&mut self, mut f: F) -> T {
        let n = self.tyvars.len();
        let r = f(self);
        let to_pop = self.tyvars.len() - n;
        self.tyvars.popn(to_pop);
        r
    }

    /// Handle namespacing. The method creates a new child namespace, enters it
    /// and then calls `f`. After `f` has returned, the current scope is returned
    /// to it's previous value
    fn with_scope<T, F: FnMut(&mut Context) -> T>(&mut self, mut f: F) -> T {
        let prev = self.current;
        let next = self.namespaces.len();
        self.namespaces.push(Namespace::with_parent(prev));
        self.current = next;
        let r = f(self);

        self.current = prev;
        r
    }

    fn define_type(&mut self, sym: Symbol, tystr: TypeStructure) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        self.types.push(tystr);
        self.namespaces[self.current].types.insert(sym, id);
        id
    }

    fn define_value(&mut self, sym: Symbol, scheme: Scheme, status: IdStatus) -> ExprId {
        let id = ExprId(self.values.len() as u32);
        self.values.push((scheme, status));
        self.namespaces[self.current].values.insert(sym, id);
        id
    }

    /// Starting from the current [`Namespace`], search for a bound name.
    /// If it's not found, then recursively search parent namespaces
    fn lookup_infix(&self, s: &Symbol) -> Option<Fixity> {
        let mut ptr = &self.namespaces[self.current];
        loop {
            match ptr.infix.get(s) {
                Some(idx) => return Some(*idx),
                None => ptr = &self.namespaces[ptr.parent?],
            }
        }
    }

    fn lookup_type(&self, sym: &Symbol) -> Option<&TypeStructure> {
        let mut ptr = &self.namespaces[self.current];
        loop {
            match ptr.types.get(sym) {
                Some(idx) => return Some(&self[*idx]),
                None => ptr = &self.namespaces[ptr.parent?],
            }
        }
    }

    fn lookup_typeid(&self, sym: &Symbol) -> Option<TypeId> {
        let mut ptr = &self.namespaces[self.current];
        loop {
            match ptr.types.get(sym) {
                Some(idx) => return Some(*idx),
                None => ptr = &self.namespaces[ptr.parent?],
            }
        }
    }

    fn lookup_value(&self, sym: &Symbol) -> Option<&(Scheme, IdStatus)> {
        let mut ptr = &self.namespaces[self.current];
        loop {
            match ptr.values.get(sym) {
                Some(idx) => return Some(&self[*idx]),
                None => ptr = &self.namespaces[ptr.parent?],
            }
        }
    }

    fn lookup_tyvar(&mut self, s: &Symbol, allow_unbound: bool) -> Option<TypeVar> {
        for (sym, tv) in self.tyvars.iter().rev() {
            if sym == s {
                return Some(tv.clone());
            }
        }
        if allow_unbound {
            Some(self.fresh_tyvar())
        } else {
            None
        }
    }

    fn fresh_tyvar(&self) -> TypeVar {
        let ex = self.tyvar_id.get();
        self.tyvar_id.set(ex + 1);
        TypeVar::new(ex, self.tyvar_rank)
    }

    fn fresh_var(&self) -> Symbol {
        let ex = self.var_id.get();
        self.var_id.set(ex + 1);
        Symbol::Gensym(ex)
    }

    fn const_ty(&self, c: &Const) -> Type {
        use super::builtin::tycons::*;
        match c {
            Const::Char(_) => Type::Con(T_CHAR, vec![]),
            Const::Int(_) => Type::Con(T_INT, vec![]),
            Const::String(_) => Type::Con(T_STRING, vec![]),
            Const::Unit => Type::Con(T_UNIT, vec![]),
        }
    }

    /// Generic function to elaborate an ast::Row<T> into a core::Row<S>
    /// where T might be ast::Type and S is core::Type, for instance
    fn elab_row<T, S, E, F: FnMut(&mut Context, &T) -> Result<S, E>>(
        &mut self,
        mut f: F,
        row: &ast::Row<T>,
    ) -> Result<Row<S>, E> {
        Ok(Row {
            label: row.label,
            span: row.span,
            data: f(self, &row.data)?,
        })
    }

    fn namespace_iter(&self) -> NamespaceIter<'_> {
        NamespaceIter {
            ctx: self,
            ptr: Some(self.current),
        }
    }
}

impl Context {
    fn elab_decl_fixity(
        &mut self,
        fixity: &ast::Fixity,
        bp: u8,
        sym: Symbol,
    ) -> Result<(), Diagnostic> {
        let fix = match fixity {
            ast::Fixity::Infix => Fixity::Infix(bp, bp + 1),
            ast::Fixity::Infixr => Fixity::Infix(bp + 1, bp),
            ast::Fixity::Nonfix => Fixity::Nonfix,
        };
        self.namespaces[self.current].infix.insert(sym, fix);
        Ok(())
    }

    fn elab_decl_local(&mut self, decls: &ast::Decl, body: &ast::Decl) -> Result<(), Diagnostic> {
        self.with_scope(|f| {
            f.elaborate_decl(decls)?;
            f.elaborate_decl(body)
        })
    }

    fn elab_decl_seq(&mut self, decls: &[ast::Decl]) -> Result<(), Diagnostic> {
        for d in decls {
            self.elaborate_decl(d)?;
        }
        Ok(())
    }

    fn elab_decl_type(&mut self, tbs: &[ast::Typebind]) -> Result<(), Diagnostic> {
        for typebind in tbs {
            let scheme = if !typebind.tyvars.is_empty() {
                self.with_tyvars(|f| {
                    for s in typebind.tyvars.iter() {
                        let v = f.fresh_tyvar();
                        f.tyvars.push(*s, v);
                    }
                    let ty = f.elaborate_type(&typebind.ty, false)?;
                    let s = match typebind.tyvars.len() {
                        0 => Scheme::Mono(ty),
                        _ => Scheme::Poly(
                            typebind
                                .tyvars
                                .iter()
                                .map(|tv| f.lookup_tyvar(tv, false).unwrap().id)
                                .collect(),
                            ty,
                        ),
                    };

                    Ok(s)
                })?
            } else {
                Scheme::Mono(self.elaborate_type(&typebind.ty, false)?)
            };
            self.define_type(typebind.tycon, TypeStructure::Scheme(scheme));
        }
        Ok(())
    }

    fn elab_decl_conbind(&mut self, db: &ast::Datatype) -> Result<(), Diagnostic> {
        let tycon = Tycon::new(db.tycon, db.tyvars.len());

        // This is safe to unwrap, because we already bound it.
        let type_id = self.lookup_typeid(&db.tycon).unwrap();

        // Should be safe to unwrap here as well, since the caller has bound db.tyvars
        let tyvars: Vec<TypeVar> = db
            .tyvars
            .iter()
            .map(|sym| self.lookup_tyvar(sym, false).unwrap())
            .collect();

        for (tag, con) in db.constructors.iter().enumerate() {
            if self.lookup_value(&con.label).is_some() {
                return Err(Diagnostic::error(
                    con.span,
                    format!(
                        "rebinding of data constructor {:?} is disallowed",
                        con.label
                    ),
                ));
            }

            let res = Type::Con(tycon, tyvars.iter().cloned().map(Type::Var).collect());
            let ty = match &con.data {
                Some(ty) => {
                    let dom = self.elaborate_type(ty, false)?;
                    Type::arrow(dom, res)
                }
                None => res,
            };

            let cons = Constructor {
                name: con.label,
                type_id,
                tag: tag as u32,
            };

            let s = match tyvars.len() {
                0 => Scheme::Mono(ty),
                _ => Scheme::Poly(tyvars.iter().map(|tv| tv.id).collect(), ty),
            };

            self.define_value(con.label, s, IdStatus::Con(cons));
        }

        Ok(())
    }

    fn elab_decl_datatype(&mut self, dbs: &[ast::Datatype]) -> Result<(), Diagnostic> {
        // Add all of the type constructors to the environment first, so that
        // we can construct data constructor arguments (e.g. recursive/mutually
        // recursive datatypes)
        for db in dbs {
            let tycon = Tycon::new(db.tycon, db.tyvars.len());
            self.define_type(db.tycon, TypeStructure::Tycon(tycon));
        }
        for db in dbs {
            self.with_tyvars(|f| {
                for s in &db.tyvars {
                    let v = f.fresh_tyvar();
                    f.tyvars.push(*s, v);
                }
                f.elab_decl_conbind(db)
            })?;
        }
        Ok(())
    }

    fn elab_decl_exception(&mut self, exns: &[ast::Variant]) -> Result<(), Diagnostic> {
        for exn in exns {
            match &exn.data {
                Some(ty) => {
                    let ty = self.elaborate_type(ty, false)?;
                    self.define_value(
                        exn.label,
                        Scheme::Mono(Type::arrow(ty, Type::exn())),
                        IdStatus::Exn(Constructor {
                            name: exn.label,
                            type_id: TypeId(8),
                            tag: 0,
                        }),
                    );
                }
                None => {
                    self.define_value(
                        exn.label,
                        Scheme::Mono(Type::exn()),
                        IdStatus::Exn(Constructor {
                            name: exn.label,
                            type_id: TypeId(8),
                            tag: 0,
                        }),
                    );
                }
            }
        }
        Ok(())
    }

    pub fn elaborate_decl(&mut self, decl: &ast::Decl) -> Result<(), Diagnostic> {
        match &decl.data {
            ast::DeclKind::Datatype(dbs) => self.elab_decl_datatype(dbs),
            ast::DeclKind::Type(tbs) => self.elab_decl_type(tbs),
            ast::DeclKind::Function(tyvars, fbs) => unimplemented!(),
            ast::DeclKind::Value(pat, expr) => {
                let expr = self.elaborate_expr(expr)?;
                let (pat, bindings) = self.elaborate_pat(pat, false)?;
                self.unify(decl.span, &pat.ty, &expr.ty)?;

                for pats::Binding { var, tv } in bindings {
                    let sch = self.generalize(Type::Var(tv));
                    println!("bind {:?} : {:?}", var, sch);
                    self.define_value(var, sch, IdStatus::Var);
                }
                Ok(())
                // dbg!(&expr);
            }
            ast::DeclKind::Exception(exns) => self.elab_decl_exception(exns),
            ast::DeclKind::Fixity(fixity, bp, sym) => self.elab_decl_fixity(fixity, *bp, *sym),
            ast::DeclKind::Local(decls, body) => self.elab_decl_local(decls, body),
            ast::DeclKind::Seq(decls) => self.elab_decl_seq(decls),
            ast::DeclKind::Do(expr) => unimplemented!(),
        }
    }
}

impl std::ops::Index<TypeId> for Context {
    type Output = TypeStructure;
    fn index(&self, index: TypeId) -> &Self::Output {
        &self.types[index.0 as usize]
    }
}

impl std::ops::Index<ExprId> for Context {
    type Output = (Scheme, IdStatus);
    fn index(&self, index: ExprId) -> &Self::Output {
        &self.values[index.0 as usize]
    }
}

pub struct NamespaceIter<'c> {
    ctx: &'c Context,
    ptr: Option<usize>,
}

impl<'c> Iterator for NamespaceIter<'c> {
    type Item = &'c Namespace;
    fn next(&mut self) -> Option<Self::Item> {
        let n = self.ptr?;
        let ns = &self.ctx.namespaces[n];
        self.ptr = ns.parent;
        Some(ns)
    }
}
