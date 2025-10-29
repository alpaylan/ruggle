use nom::{
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::char,
    character::complete::{alpha1, alphanumeric1, multispace0, multispace1},
    combinator::{eof, fail, map, not, opt, recognize, value},
    error::{ContextError, ParseError},
    multi::{many0, separated_list0},
    sequence::{delimited, pair, preceded},
    IResult,
};

use crate::query::*;

type Symbol = String;

pub fn parse_query(i: &str) -> IResult<&str, Query> {
    parse_function_query(i)
}

fn parse_symbol<'a, E>(i: &'a str) -> IResult<&'a str, Symbol, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    map(
        recognize(pair(
            alt((tag("_"), alpha1)),
            many0(alt((tag("_"), alphanumeric1))),
        )),
        |symbol: &str| symbol.to_string(),
    )(i)
}

fn parse_function_query<'a, E>(i: &'a str) -> IResult<&'a str, Query, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, qualifiers) = opt(preceded(
        multispace0,
        many0(preceded(
            multispace0,
            alt((
                tag("pub"),
                tag("async"),
                tag("unsafe"),
                tag("extern"),
                tag("const"),
                tag("fn"),
            )),
        )),
    ))(i)?;

    let qualifiers = qualifiers
        .unwrap_or_default()
        .into_iter()
        .filter_map(|q| match q {
            "async" => Some(Qualifier::Async),
            "const" => Some(Qualifier::Const),
            "unsafe" => Some(Qualifier::Unsafe),
            _ => None,
        })
        .collect::<HashSet<_>>();

    let (i, name) = opt(preceded(multispace1, parse_symbol))(i)?;
    let (i, mut decl) = opt(preceded(multispace0, parse_function))(i)?;

    if let Some(d) = decl.as_mut() {
        d.qualifiers = qualifiers;
    }

    let query = Query {
        name,
        kind: decl.map(QueryKind::FunctionQuery),
    };
    Ok((i, query))
}

fn parse_function<'a, E>(i: &'a str) -> IResult<&'a str, Function, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, decl) = parse_function_decl(i)?;

    let function = Function {
        decl,
        qualifiers: HashSet::new(),
    };
    Ok((i, function))
}

fn parse_function_decl<'a, E>(i: &'a str) -> IResult<&'a str, FnDecl, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, inputs) = delimited(
        char('('),
        alt((
            value(None, tag("..")),
            opt(parse_arguments),
            value(Some(Vec::new()), not(eof)),
        )),
        char(')'),
    )(i)?;
    let (i, output) = opt(parse_output)(i)?;

    let decl = FnDecl { inputs, output };
    Ok((i, decl))
}

fn parse_arguments<'a, E>(i: &'a str) -> IResult<&'a str, Vec<Argument>, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    separated_list0(
        char(','),
        preceded(
            multispace0,
            alt((
                parse_argument,
                value(
                    Argument {
                        ty: None,
                        name: None,
                    },
                    char('_'),
                ),
                map(parse_type, |ty| Argument {
                    ty: Some(ty),
                    name: None,
                }),
            )),
        ),
    )(i)
}

fn parse_argument<'a, E>(i: &'a str) -> IResult<&'a str, Argument, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, name) = alt((value(None, char('_')), opt(parse_symbol)))(i)?;
    let (i, _) = char(':')(i)?;
    let (i, _) = multispace0(i)?;
    let (i, ty) = alt((value(None, char('_')), opt(parse_type)))(i)?;

    let arg = Argument { ty, name };
    Ok((i, arg))
}

fn parse_output<'a, E>(i: &'a str) -> IResult<&'a str, FnRetTy, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    preceded(
        multispace0,
        alt((
            value(
                FnRetTy::DefaultReturn,
                preceded(preceded(tag("->"), multispace0), tag("()")),
            ),
            map(preceded(tag("->"), parse_type), FnRetTy::Return),
            value(FnRetTy::DefaultReturn, eof),
        )),
    )(i)
}

fn parse_type<'a, E>(i: &'a str) -> IResult<&'a str, Type, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    preceded(
        multispace0,
        alt((
            map(parse_primitive_type, Type::Primitive),
            parse_generic_type,
            parse_unresolved_path,
            parse_tuple,
            parse_slice,
            value(Type::Never, char('!')),
            parse_raw_pointer,
            parse_borrowed_ref,
        )),
    )(i)
}

fn parse_tuple<'a, E>(i: &'a str) -> IResult<&'a str, Type, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    map(
        delimited(
            char('('),
            separated_list0(
                char(','),
                preceded(
                    multispace0,
                    alt((value(None, tag("_")), map(parse_type, Some))),
                ),
            ),
            char(')'),
        ),
        Type::Tuple,
    )(i)
}

fn parse_slice<'a, E>(i: &'a str) -> IResult<&'a str, Type, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    map(
        delimited(
            char('['),
            alt((value(None, tag("_")), map(parse_type, Some))),
            char(']'),
        ),
        |ty| Type::Slice(ty.map(Box::new)),
    )(i)
}

fn parse_raw_pointer<'a, E>(i: &'a str) -> IResult<&'a str, Type, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, mutable) = alt((value(true, tag("*mut")), value(false, tag("*const"))))(i)?;
    let (i, type_) = parse_type(i)?;

    Ok((
        i,
        Type::RawPointer {
            mutable,
            type_: Box::new(type_),
        },
    ))
}

fn parse_borrowed_ref<'a, E>(i: &'a str) -> IResult<&'a str, Type, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, mutable) = alt((value(true, tag("&mut")), value(false, tag("&"))))(i)?;
    let (i, type_) = parse_type(i)?;

    Ok((
        i,
        Type::BorrowedRef {
            mutable,
            type_: Box::new(type_),
        },
    ))
}

fn parse_unresolved_path<'a, E>(i: &'a str) -> IResult<&'a str, Type, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, name) = parse_symbol(i)?;
    let (i, args) = opt(parse_generic_args)(i)?;

    Ok((
        i,
        Type::UnresolvedPath {
            name,
            args: args.map(Box::new),
        },
    ))
}

fn parse_generic_args<'a, E>(i: &'a str) -> IResult<&'a str, GenericArgs, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    map(
        delimited(
            char('<'),
            separated_list0(
                char(','),
                preceded(
                    multispace0,
                    alt((
                        value(None, tag("_")),
                        opt(map(parse_type, GenericArg::Type)),
                    )),
                ),
            ),
            char('>'),
        ),
        |args| GenericArgs::AngleBracketed { args },
    )(i)
}

fn parse_generic_type<'a, E>(i: &'a str) -> IResult<&'a str, Type, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    let (i, gen) = map(take_while1(|c: char| c.is_ascii_uppercase()), |s: &str| {
        Type::Generic(s.to_owned())
    })(i)?;

    if i.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        fail(i)
    } else {
        Ok((i, gen))
    }
}

fn parse_primitive_type<'a, E>(i: &'a str) -> IResult<&'a str, PrimitiveType, E>
where
    E: ParseError<&'a str> + ContextError<&'a str>,
{
    use PrimitiveType::*;
    alt((
        value(Isize, tag("isize")),
        value(I8, tag("i8")),
        value(I16, tag("i16")),
        value(I32, tag("i32")),
        value(I64, tag("i64")),
        value(I128, tag("i128")),
        value(Usize, tag("usize")),
        value(U8, tag("u8")),
        value(U16, tag("u16")),
        value(U32, tag("u32")),
        value(U64, tag("u64")),
        value(U128, tag("u128")),
        value(F32, tag("f32")),
        value(F64, tag("f64")),
        value(Char, tag("char")),
        value(Bool, tag("bool")),
        value(Str, tag("str")),
    ))(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_complex_type1() {
        let input = "&mut [Option<i32>]";
        let (_, ty) = parse_type::<nom::error::VerboseError<&str>>(input).unwrap();
        assert_eq!(
            ty,
            Type::BorrowedRef {
                mutable: true,
                type_: Box::new(Type::Slice(Some(Box::new(Type::UnresolvedPath {
                    name: "Option".to_string(),
                    args: Some(Box::new(GenericArgs::AngleBracketed {
                        args: vec![Some(GenericArg::Type(Type::Primitive(PrimitiveType::I32)))]
                    }))
                }))))
            }
        );
    }

    #[test]
    fn test_parse_complex_type2() {
        let input = "*const (i32, &str, T)";
        let (_, ty) = parse_type::<nom::error::VerboseError<&str>>(input).unwrap();
        assert_eq!(
            ty,
            Type::RawPointer {
                mutable: false,
                type_: Box::new(Type::Tuple(vec![
                    Some(Type::Primitive(PrimitiveType::I32)),
                    Some(Type::BorrowedRef {
                        mutable: false,
                        type_: Box::new(Type::Primitive(PrimitiveType::Str)),
                    }),
                    Some(Type::Generic("T".to_string())),
                ]))
            }
        );
    }

    #[test]
    fn test_parse_complex_type3() {
        let input = "Result<_, E>";
        let (_, ty) = parse_type::<nom::error::VerboseError<&str>>(input).unwrap();
        assert_eq!(
            ty,
            Type::UnresolvedPath {
                name: "Result".to_string(),
                args: Some(Box::new(GenericArgs::AngleBracketed {
                    args: vec![None, Some(GenericArg::Type(Type::Generic("E".to_string()))),]
                }))
            }
        );
    }

    #[test]
    fn test_parse_function_decl() {
        let input = "(x: i32, y: &str) -> bool";
        let (_, decl) = parse_function_decl::<nom::error::VerboseError<&str>>(input).unwrap();
        assert_eq!(
            decl,
            FnDecl {
                inputs: Some(vec![
                    Argument {
                        name: Some("x".to_string()),
                        ty: Some(Type::Primitive(PrimitiveType::I32)),
                    },
                    Argument {
                        name: Some("y".to_string()),
                        ty: Some(Type::BorrowedRef {
                            mutable: false,
                            type_: Box::new(Type::Primitive(PrimitiveType::Str)),
                        }),
                    },
                ]),
                output: Some(FnRetTy::Return(Type::Primitive(PrimitiveType::Bool))),
            }
        );
    }

    #[test]
    fn test_parse_function_decl_with_underscore() {
        let input = "(_, y: &str) -> ()";
        let (_, decl) = parse_function_decl::<nom::error::VerboseError<&str>>(input).unwrap();
        assert_eq!(
            decl,
            FnDecl {
                inputs: Some(vec![
                    Argument {
                        name: None,
                        ty: None,
                    },
                    Argument {
                        name: Some("y".to_string()),
                        ty: Some(Type::BorrowedRef {
                            mutable: false,
                            type_: Box::new(Type::Primitive(PrimitiveType::Str)),
                        }),
                    },
                ]),
                output: Some(FnRetTy::DefaultReturn),
            }
        );
    }

    #[test]
    fn test_parse_complex_output_type() {
        let input = "(x: i32) -> (i32, &str, T)";
        let (_, decl) = parse_function_decl::<nom::error::VerboseError<&str>>(input).unwrap();
        assert_eq!(
            decl,
            FnDecl {
                inputs: Some(vec![Argument {
                    name: Some("x".to_string()),
                    ty: Some(Type::Primitive(PrimitiveType::I32)),
                },]),
                output: Some(FnRetTy::Return(Type::Tuple(vec![
                    Some(Type::Primitive(PrimitiveType::I32)),
                    Some(Type::BorrowedRef {
                        mutable: false,
                        type_: Box::new(Type::Primitive(PrimitiveType::Str)),
                    }),
                    Some(Type::Generic("T".to_string())),
                ]))),
            }
        );
    }

    #[test]
    fn test_parse_complex_output_type2() {
        let input = "fn abc() -> Result<Vec<i32>>";
        let (_, decl) = parse_query(input).unwrap();
        assert_eq!(
            decl,
            Query {
                name: Some("abc".to_string()),
                kind: Some(QueryKind::FunctionQuery(Function {
                    decl: FnDecl {
                        inputs: Some(vec![]),
                        output: Some(FnRetTy::Return(Type::UnresolvedPath {
                            name: "Result".to_string(),
                            args: Some(Box::new(GenericArgs::AngleBracketed {
                                args: vec![Some(GenericArg::Type(Type::UnresolvedPath {
                                    name: "Vec".to_string(),
                                    args: Some(Box::new(GenericArgs::AngleBracketed {
                                        args: vec![Some(GenericArg::Type(Type::Primitive(
                                            PrimitiveType::I32
                                        )))]
                                    }))
                                }))]
                            }))
                        })),
                    },
                    qualifiers: HashSet::new(),
                })),
            }
        );
    }

    #[test]
    fn test_parse_qualified_function() {
        let input = "pub async fn foo(bar: i32, _: &str) -> bool";
        let (_, query) = parse_query(input).unwrap();
        assert_eq!(
            query,
            Query {
                name: Some("foo".to_string()),
                kind: Some(QueryKind::FunctionQuery(Function {
                    decl: FnDecl {
                        inputs: Some(vec![
                            Argument {
                                name: Some("bar".to_string()),
                                ty: Some(Type::Primitive(PrimitiveType::I32)),
                            },
                            Argument {
                                name: None,
                                ty: Some(Type::BorrowedRef {
                                    mutable: false,
                                    type_: Box::new(Type::Primitive(PrimitiveType::Str)),
                                }),
                            },
                        ]),
                        output: Some(FnRetTy::Return(Type::Primitive(PrimitiveType::Bool))),
                    },
                    qualifiers: HashSet::from_iter(vec![Qualifier::Async]),
                })),
            }
        );
    }
}
