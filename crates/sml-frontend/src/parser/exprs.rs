use super::*;

impl<'s, 'sym> Parser<'s, 'sym> {
    pub(crate) fn constant(&mut self) -> Result<Const, Error> {
        match self.bump() {
            Token::Const(c) => Ok(c),
            _ => self.error(ErrorKind::Internal),
        }
    }

    fn record_row(&mut self) -> Result<Field, Error> {
        let mut span = self.current.span;
        let label = self.expect_id()?;
        self.expect(Token::Equals)?;
        let expr = self.once(|p| p.parse_expr(), "missing expr in record row")?;
        span += self.prev;
        Ok(Field { label, expr, span })
    }

    fn record_expr(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::LBrace)?;
        let fields = self.delimited(|p| p.record_row(), Token::Comma)?;
        self.expect(Token::RBrace)?;
        Ok(ExprKind::Record(fields))
    }

    fn let_binding(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::Let)?;
        let decls = self.plus(
            |p| {
                let d = p.parse_decl();
                p.bump_if(Token::Semi);
                d
            },
            None,
        )?;
        self.expect(Token::In)?;
        let t2 = self.once(|p| p.parse_expr(), "let body required")?;
        self.expect(Token::End)?;
        Ok(ExprKind::Let(decls, Box::new(t2)))
    }

    fn case_arm(&mut self) -> Result<Arm, Error> {
        let pat = self.once(|p| p.parse_pattern(), "missing pattern in case arm")?;
        self.expect(Token::DArrow)?;
        let expr = self.once(|p| p.parse_expr(), "missing expression in case arm")?;
        self.bump_if(Token::Comma);
        Ok(Arm {
            span: pat.span + expr.span,
            pat,
            expr,
        })
    }

    fn case_expr(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::Case)?;
        let expr = self.once(|p| p.parse_expr(), "missing case expression")?;
        self.expect(Token::Of)?;
        self.bump_if(Token::Bar);
        let arms = self.delimited(|p| p.case_arm(), Token::Bar)?;
        self.expect(Token::End)?;
        Ok(ExprKind::Case(Box::new(expr), arms))
    }

    fn lambda_expr(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::Fn)?;
        let arms = self.delimited(|p| p.case_arm(), Token::Bar)?;
        Ok(ExprKind::Fn(arms))
    }

    fn while_expr(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::While)?;
        let test = self.parse_expr()?;
        self.expect(Token::Do)?;
        let expr = self.parse_expr()?;
        Ok(ExprKind::While(Box::new(test), Box::new(expr)))
    }

    fn if_expr(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::If)?;
        let test = self.parse_expr()?;
        self.expect(Token::Then)?;
        let a = self.parse_expr()?;
        self.expect(Token::Else)?;
        let b = self.parse_expr()?;
        Ok(ExprKind::If(Box::new(test), Box::new(a), Box::new(b)))
    }

    fn raise_expr(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::Raise)?;
        let expr = self.parse_expr()?;
        Ok(ExprKind::Raise(Box::new(expr)))
    }

    fn seq_expr(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::LParen)?;
        if self.bump_if(Token::RParen) {
            return Ok(ExprKind::Const(Const::Unit));
        }
        let first = self.parse_expr()?;
        let expected = match self.current() {
            Token::Semi => Token::Semi,
            Token::Comma => Token::Comma,
            _ => return Ok(first.data),
        };
        self.bump();
        let mut v = vec![first];
        while let Ok(x) = self.parse_expr() {
            v.push(x);
            if !self.bump_if(expected) {
                break;
            }
        }
        self.expect(Token::RParen)?;
        match v.len() {
            1 => Ok(v.pop().unwrap().data),
            _ => match expected {
                Token::Semi => Ok(ExprKind::Seq(v)),
                _ => Ok(make_record(v)),
            },
        }
    }

    fn selector(&mut self) -> Result<ExprKind, Error> {
        self.expect(Token::Selector)?;
        match self.current() {
            Token::Id(s) | Token::IdS(s) => {
                self.bump();
                Ok(ExprKind::Selector(s))
            }
            Token::Const(Const::Int(idx)) => {
                self.bump();
                Ok(ExprKind::Selector(Symbol::tuple_field(idx as u32)))
            }
            _ => self.error(ErrorKind::ExpectedIdentifier),
        }
    }

    /// atexp ::=   constant
    ///             id
    ///             { [label = exp] }
    ///             ()
    ///             ( exp, ... expN )
    ///             ( exp )
    ///             let decl in exp, ... expN end
    fn atomic_expr(&mut self) -> Result<Expr, Error> {
        let mut span = self.current.span;
        match self.current.data {
            Token::Id(_) | Token::IdS(_) => {
                self.expect_id().map(|e| Expr::new(ExprKind::Var(e), span))
            }
            Token::LBrace => self.spanned(|p| p.record_expr()),
            Token::Let => self.spanned(|p| p.let_binding()),
            Token::Selector => self.spanned(|p| {
                p.bump();
                p.expect_id().map(ExprKind::Selector)
            }),
            Token::Const(_) => self.constant().map(|l| Expr::new(ExprKind::Const(l), span)),
            Token::LParen => self.spanned(|p| p.seq_expr()),
            _ => self.error(ErrorKind::ExpectedExpr),
        }
    }

    /// appexp ::=      atexp
    ///                 appexp atexp
    fn application_expr(&mut self) -> Result<Expr, Error> {
        let span = self.current.span;
        let mut exprs = self.plus(|p| p.atomic_expr(), None)?;
        match exprs.len() {
            1 => Ok(exprs.pop().unwrap()),
            _ => Ok(Expr::new(ExprKind::FlatApp(exprs), span + self.prev)),
        }
    }

    /// exp ::=     if exp then exp2 else exp3
    ///             case exp of casearm end
    ///             fn x
    ///             infix
    pub fn parse_expr(&mut self) -> Result<Expr, Error> {
        let expr = match self.current() {
            Token::Case => self.spanned(|p| p.case_expr()),
            Token::Fn => self.spanned(|p| p.lambda_expr()),
            Token::While => self.spanned(|p| p.while_expr()),
            Token::If => self.spanned(|p| p.if_expr()),
            Token::Raise => self.spanned(|p| p.raise_expr()),
            _ => self.application_expr(),
        }?;

        match self.current() {
            Token::Colon => {
                self.bump();
                let snd = self.once(|p| p.parse_type(), "expected type after `exp : `")?;
                let sp = expr.span + snd.span;
                Ok(Expr::new(
                    ExprKind::Constraint(Box::new(expr), Box::new(snd)),
                    sp,
                ))
            }
            Token::Handle => {
                self.bump();
                let snd = self.spanned(|p| p.delimited(|p| p.case_arm(), Token::Bar))?;
                let sp = expr.span + snd.span;
                Ok(Expr::new(ExprKind::Handle(Box::new(expr), snd.data), sp))
            }
            Token::Orelse => {
                self.bump();
                let snd = self.once(|p| p.parse_expr(), "expected expression after orelse")?;
                let sp = expr.span + snd.span;
                Ok(Expr::new(
                    ExprKind::Orelse(Box::new(expr), Box::new(snd)),
                    sp,
                ))
            }
            Token::Andalso => {
                self.bump();
                let snd = self.once(|p| p.parse_expr(), "expected expression after andalso")?;
                let sp = expr.span + snd.span;
                Ok(Expr::new(
                    ExprKind::Andalso(Box::new(expr), Box::new(snd)),
                    sp,
                ))
            }
            _ => Ok(expr),
        }
    }
}
