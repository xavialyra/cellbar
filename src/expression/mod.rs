mod ast;
mod eval;
mod lexer;
mod parser;
mod value;

#[cfg(test)]
mod tests;

pub use ast::Ast;
#[allow(unused_imports)]
pub use eval::{escape_markup_text, evaluate, evaluate_parts, render_template_map};
pub use parser::compile;
#[allow(unused_imports)]
pub use value::DynValue;
