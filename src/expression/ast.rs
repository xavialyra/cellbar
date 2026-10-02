use super::value::DynValue;

#[derive(Clone, Debug)]
pub struct Ast {
    pub(crate) statements: Vec<Stmt>,
}

impl Ast {
    pub(crate) fn new(statements: Vec<Stmt>) -> Self {
        Self { statements }
    }

    pub(crate) fn statements(&self) -> &[Stmt] {
        &self.statements
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Stmt {
    Let(String, Expr),
    Expr(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Coalesce,
    In,
    NotIn,
}

#[derive(Clone, Debug)]
pub(crate) enum TemplateSegment {
    Literal(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub(crate) enum Expr {
    Literal(DynValue),
    Template(Vec<TemplateSegment>),
    Var(String),
    List(Vec<Expr>),
    Block(Vec<Stmt>),
    FieldAccess {
        target: Box<Expr>,
        field: String,
        optional: bool,
    },
    IndexAccess {
        target: Box<Expr>,
        index: Box<Expr>,
        optional: bool,
    },
    Call {
        func: String,
        args: Vec<Expr>,
    },
    MethodCall {
        target: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        optional: bool,
    },
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },
    UnaryNot(Box<Expr>),
    UnaryNeg(Box<Expr>),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Map(Vec<(String, Expr)>),
}
