//! Validation and SQL generation for the Formula AST.
//!
//! [`compile`] walks a [`Node`] tree, checks every operator and relationship
//! hop against the declared column types and produces a `sea_query` expression
//! that the configured backend can render. Relationship hops become correlated
//! sub-queries so that, e.g. `SUM(sales.total)` compiled against `Customer`
//! produces:
//!
//! ```text
//! (SELECT COALESCE(SUM("sale"."total"), 0) FROM "sale"
//!   WHERE "sale"."customer_id" = "customer"."id")
//! ```

use crate::sea_query::{
    BinOper, CaseStatement, Expr, ExprTrait, Func, Query, SelectStatement, Value,
};

use super::{
    AggregateFunction, BinaryOperator, FormulaType, Function, Hop, Node, NodeKind, Result,
    UnaryOperator, error::FormulaError,
};

/// A validated expression together with its type information.
struct Lowered {
    expr: Expr,
    ty: FormulaType,
    nullable: bool,
}

/// Compile a formula rooted at `root` (the table of the entity being queried).
pub fn compile(node: &Node, root: &crate::sea_query::DynIden) -> Result<Expr> {
    emit(node, root).map(|l| l.expr)
}

/// Validate a formula rooted at `root` and return the (checked) [`FormulaType`]
/// it produces, without lowering to SQL. Used by dynamic consumers to confirm a
/// formula is well-typed before executing it.
pub fn type_of(node: &Node, root: &crate::sea_query::DynIden) -> Result<FormulaType> {
    emit(node, root).map(|l| l.ty)
}

/// Lower `node`, validating it in the scope of the table `ctx`. Returns the
/// produced `sea_query` expression together with its (checked) type.
fn emit(node: &Node, ctx: &crate::sea_query::DynIden) -> Result<Lowered> {
    match node.kind() {
        NodeKind::Column {
            table,
            column,
            ty,
            nullable,
        } => {
            if table != ctx {
                return Err(FormulaError::new(format!(
                    "column `{table}.{column}` is not reachable from the current \
                     scope `{ctx}`: it must be accessed through a relationship hop"
                )));
            }
            Ok(Lowered {
                expr: Expr::col((table.clone(), column.clone())),
                ty: *ty,
                nullable: *nullable,
            })
        }

        NodeKind::Literal { value, ty } => {
            if *ty == FormulaType::Null {
                Ok(Lowered {
                    expr: Expr::cust("NULL"),
                    ty: FormulaType::Null,
                    nullable: true,
                })
            } else {
                Ok(Lowered {
                    expr: Expr::value(value.clone()),
                    ty: *ty,
                    nullable: false,
                })
            }
        }

        NodeKind::Unary { op, operand } => {
            let l = emit(operand, ctx)?;
            match op {
                UnaryOperator::Neg => {
                    if !l.ty.is_numeric() {
                        return Err(FormulaError::type_mismatch(
                            "negation",
                            "operand",
                            FormulaType::Decimal,
                            l.ty,
                        ));
                    }
                    let zero = Expr::val(Value::Int(Some(0)));
                    Ok(Lowered {
                        expr: zero.binary(BinOper::Sub, l.expr),
                        ty: l.ty,
                        nullable: l.nullable,
                    })
                }
                UnaryOperator::Not => {
                    require_boolean("NOT", &l)?;
                    Ok(Lowered {
                        expr: l.expr.not(),
                        ty: FormulaType::Boolean,
                        nullable: l.nullable,
                    })
                }
            }
        }

        NodeKind::Binary { op, left, right } => {
            let l = emit(left, ctx)?;
            let r = emit(right, ctx)?;
            match op {
                BinaryOperator::Add
                | BinaryOperator::Sub
                | BinaryOperator::Mul
                | BinaryOperator::Div
                | BinaryOperator::Rem => {
                    let name = arith_name(*op);
                    if !l.ty.is_numeric() || !r.ty.is_numeric() {
                        return Err(FormulaError::new(format!(
                            "cannot apply arithmetic operator `{name}` to operands of type \
                             `{}` and `{}`",
                            l.ty, r.ty
                        )));
                    }
                    let res = arith_result_type(*op, l.ty, r.ty);
                    let expr = arith_expr(*op, &l.expr, &r.expr);
                    Ok(Lowered {
                        expr,
                        ty: res,
                        nullable: l.nullable || r.nullable,
                    })
                }
                BinaryOperator::Eq
                | BinaryOperator::NotEq
                | BinaryOperator::Lt
                | BinaryOperator::LtEq
                | BinaryOperator::Gt
                | BinaryOperator::GtEq => {
                    let name = cmp_name(*op);
                    if !comparable(l.ty, r.ty) {
                        return Err(FormulaError::incompatible(name, l.ty, r.ty));
                    }
                    Ok(Lowered {
                        expr: cmp_expr(*op, &l.expr, &r.expr),
                        ty: FormulaType::Boolean,
                        nullable: l.nullable || r.nullable,
                    })
                }
                BinaryOperator::And | BinaryOperator::Or => {
                    require_boolean("AND/OR", &l)?;
                    require_boolean("AND/OR", &r)?;
                    let expr = match op {
                        BinaryOperator::And => l.expr.binary(BinOper::And, r.expr),
                        _ => l.expr.binary(BinOper::Or, r.expr),
                    };
                    Ok(Lowered {
                        expr,
                        ty: FormulaType::Boolean,
                        nullable: l.nullable || r.nullable,
                    })
                }
            }
        }

        NodeKind::IsNull { operand, negate } => {
            let l = emit(operand, ctx)?;
            let expr = if *negate {
                l.expr.is_not_null()
            } else {
                l.expr.is_null()
            };
            Ok(Lowered {
                expr,
                ty: FormulaType::Boolean,
                nullable: false,
            })
        }

        NodeKind::Cast { operand, target } => {
            let l = emit(operand, ctx)?;
            let expr = l.expr.cast_as(sql_type_name(*target));
            Ok(Lowered {
                expr,
                ty: *target,
                nullable: l.nullable,
            })
        }

        NodeKind::Case { arms, r#else } => {
            let mut cs: Option<CaseStatement> = None;
            for (cond, val) in arms {
                let c = emit(cond, ctx)?;
                require_boolean("CASE WHEN", &c)?;
                let v = emit(val, ctx)?;
                let case = match cs.take() {
                    None => Expr::case(c.expr, v.expr),
                    Some(existing) => existing.case(c.expr, v.expr),
                };
                cs = Some(case);
            }
            let el = emit(r#else, ctx)?;
            let mut case = cs.take().expect("case must have at least one arm");
            case = case.finally(el.expr);
            Ok(Lowered {
                expr: case.into(),
                ty: el.ty,
                nullable: el.nullable,
            })
        }

        NodeKind::Function { fun, args } => lower_function(*fun, args, ctx),

        NodeKind::One { hop, inner } => lower_one(hop, inner, ctx),

        NodeKind::Aggregate {
            hop,
            fun,
            arg,
            filter,
        } => lower_aggregate(hop, *fun, arg, filter.as_deref(), ctx),
    }
}

fn require_boolean(op: &str, l: &Lowered) -> Result<()> {
    if l.ty != FormulaType::Boolean {
        return Err(FormulaError::type_mismatch(
            op,
            "condition",
            FormulaType::Boolean,
            l.ty,
        ));
    }
    Ok(())
}

fn arith_name(op: BinaryOperator) -> &'static str {
    match op {
        BinaryOperator::Add => "+",
        BinaryOperator::Sub => "-",
        BinaryOperator::Mul => "*",
        BinaryOperator::Div => "/",
        BinaryOperator::Rem => "%",
        _ => unreachable!(),
    }
}

fn cmp_name(op: BinaryOperator) -> &'static str {
    match op {
        BinaryOperator::Eq => "=",
        BinaryOperator::NotEq => "<>",
        BinaryOperator::Lt => "<",
        BinaryOperator::LtEq => "<=",
        BinaryOperator::Gt => ">",
        BinaryOperator::GtEq => ">=",
        _ => unreachable!(),
    }
}

fn comparable(a: FormulaType, b: FormulaType) -> bool {
    if a.is_numeric() && b.is_numeric() {
        return true;
    }
    if a == FormulaType::Null || b == FormulaType::Null {
        return true;
    }
    a == b
}

fn arith_result_type(op: BinaryOperator, lt: FormulaType, rt: FormulaType) -> FormulaType {
    // Integer division is not used: to preserve money semantics we coerce the
    // result of `/` to decimal unless both operands are floating point.
    if op == BinaryOperator::Div {
        if lt == FormulaType::Float && rt == FormulaType::Float {
            return FormulaType::Float;
        }
        return FormulaType::Decimal;
    }
    FormulaType::promote_numeric(lt, rt).unwrap_or(lt)
}

fn arith_expr(op: BinaryOperator, l: &Expr, r: &Expr) -> Expr {
    use BinOper::*;
    let bin = match op {
        BinaryOperator::Add => Add,
        BinaryOperator::Sub => Sub,
        BinaryOperator::Mul => Mul,
        BinaryOperator::Div => Div,
        BinaryOperator::Rem => Mod,
        _ => unreachable!(),
    };
    l.clone().binary(bin, r.clone())
}

fn cmp_expr(op: BinaryOperator, l: &Expr, r: &Expr) -> Expr {
    use BinOper::*;
    let bin = match op {
        BinaryOperator::Eq => Equal,
        BinaryOperator::NotEq => NotEqual,
        BinaryOperator::Lt => SmallerThan,
        BinaryOperator::LtEq => SmallerThanOrEqual,
        BinaryOperator::Gt => GreaterThan,
        BinaryOperator::GtEq => GreaterThanOrEqual,
        _ => unreachable!(),
    };
    l.clone().binary(bin, r.clone())
}

/// Lower scalar function calls.
fn lower_function(
    fun: Function,
    args: &[Node],
    ctx: &crate::sea_query::DynIden,
) -> Result<Lowered> {
    match fun {
        Function::Coalesce => {
            let mut lowered = Vec::new();
            let mut ty = FormulaType::Null;
            for a in args {
                let l = emit(a, ctx)?;
                ty = merge_type(ty, l.ty);
                lowered.push(l.expr);
            }
            if ty == FormulaType::Null {
                ty = FormulaType::String;
            }
            Ok(Lowered {
                expr: Func::coalesce(lowered).into(),
                ty,
                nullable: false,
            })
        }
        Function::NullIf => {
            if args.len() != 2 {
                return Err(FormulaError::new("NULLIF requires two arguments"));
            }
            let a = emit(&args[0], ctx)?;
            let b = emit(&args[1], ctx)?;
            if !comparable(a.ty, b.ty) {
                return Err(FormulaError::incompatible("NULLIF", a.ty, b.ty));
            }
            // NULLIF(a, b) == CASE WHEN a = b THEN NULL ELSE a END
            let case = Expr::case(
                a.expr.clone().binary(BinOper::Equal, b.expr.clone()),
                Expr::cust("NULL"),
            )
            .finally(a.expr.clone());
            Ok(Lowered {
                expr: case.into(),
                ty: a.ty,
                nullable: true,
            })
        }
        Function::Abs => {
            let a = emit(&args[0], ctx)?;
            if !a.ty.is_numeric() {
                return Err(FormulaError::type_mismatch(
                    "ABS",
                    "argument",
                    FormulaType::Decimal,
                    a.ty,
                ));
            }
            Ok(Lowered {
                expr: Func::abs(a.expr).into(),
                ty: a.ty,
                nullable: a.nullable,
            })
        }
        Function::Lower => single_string_arg("LOWER", args, ctx, |e| Func::lower(e).into()),
        Function::Upper => single_string_arg("UPPER", args, ctx, |e| Func::upper(e).into()),
        Function::Length => {
            let a = emit(&args[0], ctx)?;
            Ok(Lowered {
                expr: Func::cust("LENGTH").arg(a.expr).into(),
                ty: FormulaType::Integer,
                nullable: a.nullable,
            })
        }
        Function::Round => {
            let a = emit(&args[0], ctx)?;
            Ok(Lowered {
                expr: Func::round(a.expr).into(),
                ty: a.ty,
                nullable: a.nullable,
            })
        }
        Function::Floor => {
            let a = emit(&args[0], ctx)?;
            Ok(Lowered {
                expr: Func::cust("FLOOR").arg(a.expr).into(),
                ty: a.ty,
                nullable: a.nullable,
            })
        }
        Function::Ceil => {
            let a = emit(&args[0], ctx)?;
            Ok(Lowered {
                expr: Func::cust("CEIL").arg(a.expr).into(),
                ty: a.ty,
                nullable: a.nullable,
            })
        }
        Function::Concat => {
            let mut lowered = Vec::new();
            let mut nullable = false;
            for a in args {
                let l = emit(a, ctx)?;
                nullable |= l.nullable;
                lowered.push(l.expr);
            }
            Ok(Lowered {
                expr: Func::cust("CONCAT").args(lowered).into(),
                ty: FormulaType::String,
                nullable,
            })
        }
        Function::CurrentDate => Ok(Lowered {
            expr: Expr::current_date(),
            ty: FormulaType::Date,
            nullable: false,
        }),
    }
}

fn single_string_arg(
    name: &str,
    args: &[Node],
    ctx: &crate::sea_query::DynIden,
    build: impl FnOnce(Expr) -> Expr,
) -> Result<Lowered> {
    if args.len() != 1 {
        return Err(FormulaError::new(format!("{name} requires one argument")));
    }
    let a = emit(&args[0], ctx)?;
    if a.ty != FormulaType::String {
        return Err(FormulaError::type_mismatch(
            name,
            "argument",
            FormulaType::String,
            a.ty,
        ));
    }
    Ok(Lowered {
        expr: build(a.expr),
        ty: a.ty,
        nullable: a.nullable,
    })
}

fn merge_type(a: FormulaType, b: FormulaType) -> FormulaType {
    if a == FormulaType::Null {
        return b;
    }
    if b == FormulaType::Null {
        return a;
    }
    FormulaType::promote_numeric(a, b).unwrap_or_else(|| {
        if a == b {
            a
        } else {
            // Fall back to string for heterogeneous COALESCE (e.g. string + null)
            FormulaType::String
        }
    })
}

/// Lower a correlated scalar subquery for a `belongs_to` / `has_one` hop.
fn lower_one(hop: &Hop, inner: &Node, ctx: &crate::sea_query::DynIden) -> Result<Lowered> {
    if &hop.source != ctx {
        return Err(FormulaError::new(format!(
            "relationship `{}` does not start from the current scope `{}`",
            hop.source, ctx
        )));
    }
    let inner_expr = emit(inner, &hop.target)?;

    let mut sub = Query::select();
    sub.expr(inner_expr.expr.clone()).from(hop.target.clone());
    apply_correlation(&mut sub, hop);

    Ok(Lowered {
        expr: subquery_expr(sub),
        ty: inner_expr.ty,
        nullable: true,
    })
}

/// Lower a correlated aggregate over a to-many collection.
fn lower_aggregate(
    hop: &Hop,
    fun: AggregateFunction,
    arg: &Node,
    filter: Option<&Node>,
    ctx: &crate::sea_query::DynIden,
) -> Result<Lowered> {
    if &hop.source != ctx {
        return Err(FormulaError::new(format!(
            "relationship `{}` does not start from the current scope `{}`",
            hop.source, ctx
        )));
    }
    let arg_l = emit(arg, &hop.target)?;

    let agg_expr = aggregate_expr(fun, &arg_l.expr);
    let result_ty = aggregate_type(fun, arg_l.ty);

    let mut sub = Query::select();
    sub.expr(agg_expr).from(hop.target.clone());
    apply_correlation(&mut sub, hop);

    if let Some(f) = filter {
        let f_l = emit(f, &hop.target)?;
        require_boolean("aggregate filter", &f_l)?;
        sub.and_where(f_l.expr);
    }

    Ok(Lowered {
        expr: subquery_expr(sub),
        ty: result_ty,
        nullable: true,
    })
}

/// Turn a [`SelectStatement`] into a scalar sub-query expression.
fn subquery_expr(sub: SelectStatement) -> Expr {
    use crate::sea_query::QueryStatementBuilder;
    let stmt = sub.into_sub_query_statement();
    Expr::from(stmt)
}

/// Add the `target.col == source.col` correlation predicates to a sub-query.
fn apply_correlation(sub: &mut SelectStatement, hop: &Hop) {
    for (i, tgt_col) in hop.target_cols.iter().enumerate() {
        let src_col = &hop.source_cols[i];
        let lhs = Expr::col((hop.target.clone(), tgt_col.clone()));
        let rhs = Expr::col((hop.source.clone(), src_col.clone()));
        sub.and_where(lhs.eq(rhs));
    }
}

fn aggregate_expr(fun: AggregateFunction, arg: &Expr) -> Expr {
    match fun {
        AggregateFunction::Sum => arg.clone().sum(),
        AggregateFunction::Avg => arg.clone().avg(),
        AggregateFunction::Min => arg.clone().min(),
        AggregateFunction::Max => arg.clone().max(),
        AggregateFunction::Count => arg.clone().count(),
    }
}

fn aggregate_type(fun: AggregateFunction, arg: FormulaType) -> FormulaType {
    match fun {
        AggregateFunction::Sum => {
            FormulaType::promote_numeric(arg, FormulaType::Decimal).unwrap_or(FormulaType::Integer)
        }
        AggregateFunction::Avg => {
            if arg == FormulaType::Float {
                FormulaType::Float
            } else {
                FormulaType::Decimal
            }
        }
        AggregateFunction::Min | AggregateFunction::Max => arg,
        AggregateFunction::Count => FormulaType::Integer,
    }
}

fn sql_type_name(ty: FormulaType) -> &'static str {
    match ty {
        FormulaType::Boolean => "boolean",
        FormulaType::Integer => "bigint",
        FormulaType::Decimal => "numeric",
        FormulaType::Float => "float",
        FormulaType::String => "text",
        FormulaType::Date => "date",
        FormulaType::DateTime => "timestamp",
        FormulaType::Uuid => "uuid",
        FormulaType::Json => "json",
        FormulaType::Null => "text",
    }
}
