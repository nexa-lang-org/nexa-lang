use crate::domain::ast::Type;

pub fn type_to_js(ty: &Type) -> String {
    match ty {
        Type::Int => "number".into(),
        Type::String => "string".into(),
        Type::Bool => "boolean".into(),
        Type::Void => "void".into(),
        Type::Custom(name) | Type::Generic(name) => name.clone(),
        Type::List(inner) => format!("Array<{}>", type_to_js(inner)),
        Type::Function(params, ret) => {
            let p: Vec<_> = params.iter().map(type_to_js).collect();
            format!("({}) => {}", p.join(", "), type_to_js(ret))
        }
    }
}

pub fn types_compatible(a: &Type, b: &Type) -> bool {
    a == b
        || matches!(
            (a, b),
            (Type::Custom(_), _)
                | (_, Type::Custom(_))
                | (Type::Generic(_), _)
                | (_, Type::Generic(_))
        )
}
