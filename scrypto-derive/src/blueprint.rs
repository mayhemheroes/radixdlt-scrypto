use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::*;

use crate::ast;

macro_rules! trace {
    ($($arg:expr),*) => {{
        #[cfg(feature = "trace")]
        println!($($arg),*);
    }};
}

pub fn handle_blueprint(input: TokenStream) -> Result<TokenStream> {
    trace!("Started processing blueprint macro");

    // parse blueprint struct and impl
    let bp = parse2::<ast::Blueprint>(input)?;
    let bp_strut = &bp.structure;
    let bp_fields = &bp_strut.fields;
    let bp_semi_token = &bp_strut.semi_token;
    let bp_impl = &bp.implementation;
    let bp_ident = &bp_strut.ident;
    let bp_items = &bp_impl.items;
    let bp_name = bp_ident.to_string();
    trace!("Blueprint name: {}", bp_name);

    let impl_ident_matches = match &*bp_impl.self_ty {
        Type::Path(p) => p
            .path
            .get_ident()
            .filter(|i| i.to_string() == bp_name)
            .is_some(),
        _ => false,
    };
    if !impl_ident_matches {
        return Err(Error::new(
            bp_impl.span(),
            format!("Only `impl {}` is allowed here", bp_name),
        ));
    }

    let module_ident = format_ident!("{}_impl", bp_ident);
    let value_ident = format_ident!("{}Component", bp_ident);

    let output_mod = quote! {
        #[allow(non_snake_case)]
        pub mod #module_ident {
            use super::*;

            #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
            pub struct #bp_ident #bp_fields #bp_semi_token

            impl #bp_ident {
                #(#bp_items)*
            }

            impl ::scrypto::component::ComponentState<#value_ident> for #bp_ident {
                fn instantiate(self) -> #value_ident {
                    let component = ::scrypto::component::component_system().create_component(
                        #bp_name,
                        self
                    );
                    #value_ident {
                        component
                    }
                }
            }
        }
    };
    trace!("Generated mod: \n{}", quote! { #output_mod });
    let method_input_structs = generate_method_input_structs(bp_ident, bp_items);

    let functions = generate_dispatcher(&module_ident, bp_ident, bp_items)?;
    let output_dispatcher = quote! {
        #(#method_input_structs)*
        #(#functions)*
    };

    trace!("Generated dispatcher: \n{}", quote! { #output_dispatcher });

    let abi_ident = format_ident!("{}_abi", bp_ident);
    let abi_functions = generate_abi(bp_ident, bp_items)?;
    let output_abi = quote! {
        #[no_mangle]
        pub extern "C" fn #abi_ident(input: *mut u8) -> *mut u8 {
            use ::sbor::{Describe, Type};
            use ::scrypto::abi::{BlueprintAbi, Fn};
            use ::sbor::rust::borrow::ToOwned;
            use ::sbor::rust::vec;
            use ::sbor::rust::vec::Vec;

            let fns: Vec<Fn> = vec![ #(#abi_functions),* ];
            let structure: Type = #module_ident::#bp_ident::describe();
            let output = BlueprintAbi {
                structure,
                fns,
            };

            ::scrypto::buffer::scrypto_encode_to_buffer(&output)
        }
    };
    trace!(
        "Generated ABI exporter: \n{}",
        quote! { #output_dispatcher }
    );

    let output_stubs = generate_stubs(&value_ident, bp_ident, bp_items)?;

    let output = quote! {
        #output_mod

        #output_dispatcher

        #output_abi

        #output_stubs
    };
    trace!("Finished processing blueprint macro");

    #[cfg(feature = "trace")]
    crate::utils::print_generated_code("blueprint!", &output);

    Ok(output)
}

fn generate_method_input_structs(bp_ident: &Ident, items: &[ImplItem]) -> Vec<ItemStruct> {
    let mut method_input_structs = Vec::new();

    for item in items {
        if let ImplItem::Method(method) = item {
            if !matches!(method.vis, Visibility::Public(_)) {
                continue;
            }

            let mut args = Vec::new();
            let mut index: usize = 0;
            for input in (&method.sig.inputs).into_iter() {
                match input {
                    FnArg::Receiver(_) => {}
                    FnArg::Typed(ref t) => {
                        let arg_ident = format_ident!("arg{}", index);
                        index += 1;
                        let arg_type = t.ty.as_ref();
                        let arg: Field = Field::parse_named
                            .parse2(quote! {
                                #arg_ident : #arg_type
                            })
                            .unwrap();
                        args.push(arg)
                    }
                }
            }

            let input_struct_name = format_ident!("{}_{}_Input", bp_ident, method.sig.ident);

            let method_input_struct: ItemStruct = parse_quote! {
                #[allow(non_camel_case_types)]
                #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
                pub struct #input_struct_name {
                    #(#args),*
                }
            };
            method_input_structs.push(method_input_struct);
        }
    }
    method_input_structs
}

// Parses function items in an `Impl` and returns the arm guards and bodies
// used for call matching.
fn generate_dispatcher(
    module_ident: &Ident,
    bp_ident: &Ident,
    items: &[ImplItem],
) -> Result<Vec<TokenStream>> {
    let mut functions = Vec::new();

    for item in items {
        trace!("Processing item: {}", quote! { #item });

        if let ImplItem::Method(ref m) = item {
            if let Visibility::Public(_) = &m.vis {
                let ident = &m.sig.ident;

                let mut match_args: Vec<Expr> = vec![];
                let mut dispatch_args: Vec<Expr> = vec![];
                let mut stmts: Vec<Stmt> = vec![];
                let mut get_state: Option<Stmt> = None;
                for (i, input) in (&m.sig.inputs).into_iter().enumerate() {
                    match input {
                        FnArg::Receiver(ref r) => {
                            // Check receiver type and mutability
                            if r.reference.is_none() {
                                return Err(Error::new(r.span(), "Function input `self` is not supported. Try replacing it with `&self`."));
                            }
                            let mutability = r.mutability;

                            // Generate a `Stmt` for loading the component state
                            assert!(get_state.is_none(), "Can't have more than 1 self reference");
                            if mutability.is_some() {
                                // Generate an `Arg` and a loading `Stmt` for the i-th argument
                                dispatch_args.push(parse_quote! { state.deref_mut() });
                                get_state = Some(parse_quote! {
                                    let mut state: DataRefMut<#module_ident::#bp_ident> = component_data.get_mut();
                                });
                            } else {
                                dispatch_args.push(parse_quote! { state.deref() });
                                get_state = Some(parse_quote! {
                                    let state: DataRef<#module_ident::#bp_ident> = component_data.get();
                                });
                            }
                        }
                        FnArg::Typed(_) => {
                            let arg_index = if get_state.is_some() { i - 1 } else { i };
                            let arg = format_ident!("arg{}", arg_index);

                            match_args.push(parse_quote! { #arg });
                            dispatch_args.push(parse_quote! { input.#arg });
                        }
                    }
                }

                // parse input
                let input_struct_name = format_ident!("{}_{}_Input", bp_ident, ident);
                stmts.push(parse_quote!{
                    let input: #input_struct_name = ::scrypto::buffer::scrypto_decode_from_buffer(args).unwrap();
                });

                // load state if needed
                if let Some(stmt) = get_state {
                    trace!("Generated stmt: {}", quote! { #stmt });
                    stmts.push(parse_quote! {
                        let actor = ::scrypto::core::Runtime::actor();
                    });
                    stmts.push(parse_quote! {
                        let (component_address, ..) = actor.as_component();
                    });
                    stmts.push(parse_quote!{
                        let mut component_data = ::scrypto::core::DataPointer::new(::scrypto::engine::types::SubstateId::ComponentState(component_address));
                    });
                    stmts.push(stmt);
                }

                // call the function
                let stmt: Stmt = parse_quote! {
                    let rtn = ::scrypto::buffer::scrypto_encode_to_buffer(
                        &#module_ident::#bp_ident::#ident(#(#dispatch_args),*)
                    );
                };
                trace!("Generated stmt: {}", quote! { #stmt });
                stmts.push(stmt);

                // update state
                stmts.push(Stmt::Expr(parse_quote! { rtn }));

                let fn_ident = format_ident!("{}_{}", bp_ident, ident);
                let extern_function = quote! {
                    #[no_mangle]
                    pub extern "C" fn #fn_ident(args: *mut u8) -> *mut u8 {
                        use ::sbor::rust::ops::{Deref, DerefMut};

                        // Set up panic hook
                        ::scrypto::misc::set_up_panic_hook();

                        // Set up component and resource subsystems;
                        ::scrypto::component::init_component_system(::scrypto::component::ComponentSystem::new());
                        ::scrypto::resource::init_resource_system(::scrypto::resource::ResourceSystem::new());

                        #(#stmts)*
                    }
                };
                functions.push(extern_function);
            }
        };
    }

    Ok(functions)
}

// Parses function items of an `Impl` and returns ABI of functions.
fn generate_abi(bp_ident: &Ident, items: &[ImplItem]) -> Result<Vec<Expr>> {
    let mut fns = Vec::<Expr>::new();

    for item in items {
        trace!("Processing item: {}", quote! { #item });
        match item {
            ImplItem::Method(ref m) => {
                if let Visibility::Public(_) = &m.vis {
                    let name = m.sig.ident.to_string();
                    let mut mutability = None;
                    let mut inputs = vec![];
                    for input in &m.sig.inputs {
                        match input {
                            FnArg::Receiver(ref r) => {
                                // Check receiver type and mutability
                                if r.reference.is_none() {
                                    return Err(Error::new(r.span(), "Function input `self` is not supported. Try replacing it with &self."));
                                }

                                if r.mutability.is_some() {
                                    mutability =
                                        Some(quote! { ::scrypto::abi::SelfMutability::Mutable });
                                } else {
                                    mutability =
                                        Some(quote! { ::scrypto::abi::SelfMutability::Immutable });
                                }
                            }
                            FnArg::Typed(ref t) => {
                                let ty = replace_self_with(&t.ty, &bp_ident.to_string());
                                inputs.push(quote! {
                                    <#ty>::describe()
                                });
                            }
                        }
                    }

                    let input_struct_name = format_ident!("{}_{}_Input", bp_ident, m.sig.ident);
                    let input = quote! {
                        #input_struct_name::describe()
                    };
                    let output = match &m.sig.output {
                        ReturnType::Default => quote! {
                            ::sbor::describe::Type::Unit
                        },
                        ReturnType::Type(_, t) => {
                            let ty = replace_self_with(t, &bp_ident.to_string());
                            quote! {
                                <#ty>::describe()
                            }
                        }
                    };
                    let export_name = format!("{}_{}", bp_ident, m.sig.ident);

                    if mutability.is_none() {
                        fns.push(parse_quote! {
                            ::scrypto::abi::Fn {
                                ident: #name.to_owned(),
                                mutability: Option::None,
                                input: #input,
                                output: #output,
                                export_name: #export_name.to_string(),
                            }
                        });
                    } else {
                        fns.push(parse_quote! {
                            ::scrypto::abi::Fn {
                                ident: #name.to_owned(),
                                mutability: Option::Some(#mutability),
                                input: #input,
                                output: #output,
                                export_name: #export_name.to_string(),
                            }
                        });
                    }
                }
            }
            _ => {
                return Err(Error::new(
                    item.span(),
                    "Non-method impl items are not supported!",
                ));
            }
        };
    }

    Ok(fns)
}

// Parses function items of an `Impl` and returns ABI of functions.
fn generate_stubs(
    value_ident: &Ident,
    bp_ident: &Ident,
    items: &[ImplItem],
) -> Result<TokenStream> {
    let bp_name = bp_ident.to_string();
    let mut functions = Vec::<ImplItem>::new();
    let mut methods = Vec::<ImplItem>::new();

    for item in items {
        trace!("Processing item: {}", quote! { #item });
        match item {
            ImplItem::Method(ref m) => {
                if let Visibility::Public(_) = &m.vis {
                    let ident = &m.sig.ident;
                    let name = ident.to_string();
                    let mut mutable = None;
                    let mut input_types = vec![];
                    let mut input_args = vec![];
                    let mut input_len = 0;
                    for input in &m.sig.inputs {
                        match input {
                            FnArg::Receiver(ref r) => {
                                // Check receiver type and mutability
                                if r.reference.is_none() {
                                    return Err(Error::new(r.span(), "Function input `self` is not supported. Try replacing it with &self."));
                                }

                                if r.mutability.is_some() {
                                    mutable = Some(true);
                                } else {
                                    mutable = Some(false);
                                }
                            }
                            FnArg::Typed(ref t) => {
                                let arg = format_ident!("arg{}", input_len.to_string());
                                input_args.push(arg);

                                let ty = replace_self_with(&t.ty, &bp_ident.to_string());
                                input_types.push(ty);

                                input_len += 1;
                            }
                        }
                    }

                    let output = match &m.sig.output {
                        ReturnType::Default => parse_quote! { () },
                        ReturnType::Type(_, t) => replace_self_with(t, &bp_ident.to_string()),
                    };

                    if mutable.is_none() {
                        functions.push(parse_quote! {
                            pub fn #ident(#(#input_args: #input_types),*) -> #output {
                                ::scrypto::core::Runtime::call_function(
                                    ::scrypto::core::Runtime::package_address(),
                                    #bp_name,
                                    #name,
                                    ::scrypto::args!(#(#input_args),*)
                                )
                            }
                        });
                    } else {
                        methods.push(parse_quote! {
                            pub fn #ident(&self #(, #input_args: #input_types)*) -> #output {
                                self.component.call(#name, ::scrypto::args!(
                                    #(
                                       #input_args
                                    ),*
                                ))
                            }
                        });
                    }
                }
            }
            _ => {
                return Err(Error::new(
                    item.span(),
                    "Non-method impl items are not supported!",
                ));
            }
        };
    }

    let output = quote! {
        #[allow(non_camel_case_types)]
        #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
        pub struct #value_ident {
            pub component: ::scrypto::component::Component,
        }

        impl ::scrypto::component::LocalComponent for #value_ident {
            fn package_address(&self) -> PackageAddress {
                self.component.package_address()
            }
            fn blueprint_name(&self) -> String {
                self.component.blueprint_name()
            }
            fn add_access_check(&mut self, access_rules: ::scrypto::resource::AccessRules) -> &mut Self {
                self.component.add_access_check(access_rules);
                self
            }
            fn globalize(self) -> ComponentAddress {
                self.component.globalize()
            }
        }

        impl #value_ident {
            #(#functions)*

            #(#methods)*
        }
    };

    Ok(output)
}

fn replace_self_with(t: &Type, name: &str) -> Type {
    match t {
        Type::Path(tp) => {
            let mut tp2 = tp.clone();
            tp2.path.segments.iter_mut().for_each(|s| {
                if s.ident == "Self" {
                    s.ident = format_ident!("{}", name)
                }
            });
            Type::Path(tp2)
        }
        _ => t.clone(),
    }
}

#[cfg(test)]
mod tests {
    use proc_macro2::TokenStream;
    use std::str::FromStr;

    use super::*;

    fn assert_code_eq(a: TokenStream, b: TokenStream) {
        assert_eq!(a.to_string(), b.to_string());
    }

    #[test]
    #[should_panic]
    fn test_inconsistent_names_should_fail() {
        let input = TokenStream::from_str("struct A {} impl B { }").unwrap();
        handle_blueprint(input).unwrap();
    }

    #[test]
    fn test_blueprint() {
        let input = TokenStream::from_str(
            "struct Test {a: u32, admin: ResourceManager} impl Test { pub fn x(&self, i: u32) -> u32 { i + self.a } pub fn y(i: u32) -> u32 { i * 2 } }",
        )
        .unwrap();
        let output = handle_blueprint(input).unwrap();

        assert_code_eq(
            output,
            quote! {
                #[allow(non_snake_case)]
                pub mod Test_impl {
                    use super::*;

                    #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
                    pub struct Test {
                        a: u32,
                        admin: ResourceManager
                    }

                    impl Test {
                        pub fn x(&self, i: u32) -> u32 {
                            i + self.a
                        }
                        pub fn y(i: u32) -> u32 {
                            i * 2
                        }
                    }

                    impl ::scrypto::component::ComponentState<TestComponent> for Test {
                        fn instantiate(self) -> TestComponent {
                            let component = ::scrypto::component::component_system().create_component(
                                "Test",
                                self
                            );
                            TestComponent {
                                component
                            }
                        }
                    }
                }

                #[allow(non_camel_case_types)]
                #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
                pub struct Test_x_Input { arg0 : u32 }

                #[allow(non_camel_case_types)]
                #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
                pub struct Test_y_Input { arg0 : u32 }

                #[no_mangle]
                pub extern "C" fn Test_x(args: *mut u8) -> *mut u8 {
                    use ::sbor::rust::ops::{Deref, DerefMut};

                    // Set up panic hook
                    ::scrypto::misc::set_up_panic_hook();

                    // Set up component and resource subsystems;
                    ::scrypto::component::init_component_system(::scrypto::component::ComponentSystem::new());
                    ::scrypto::resource::init_resource_system(::scrypto::resource::ResourceSystem::new());

                    let input: Test_x_Input = ::scrypto::buffer::scrypto_decode_from_buffer(args).unwrap();
                    let actor = ::scrypto::core::Runtime::actor();
                    let (component_address, ..) = actor.as_component();
                    let mut component_data = ::scrypto::core::DataPointer::new(::scrypto::engine::types::SubstateId::ComponentState(component_address));
                    let state: DataRef<Test_impl::Test> = component_data.get();

                    let rtn = ::scrypto::buffer::scrypto_encode_to_buffer(&Test_impl::Test::x(state.deref(), input.arg0));
                    rtn
                }

                #[no_mangle]
                pub extern "C" fn Test_y(args: *mut u8) -> *mut u8 {
                    use ::sbor::rust::ops::{Deref, DerefMut};

                    // Set up panic hook
                    ::scrypto::misc::set_up_panic_hook();

                    // Set up component and resource subsystems;
                    ::scrypto::component::init_component_system(::scrypto::component::ComponentSystem::new());
                    ::scrypto::resource::init_resource_system(::scrypto::resource::ResourceSystem::new());

                    let input: Test_y_Input = ::scrypto::buffer::scrypto_decode_from_buffer(args).unwrap();
                    let rtn = ::scrypto::buffer::scrypto_encode_to_buffer(&Test_impl::Test::y(input.arg0));
                    rtn
                }

                #[no_mangle]
                pub extern "C" fn Test_abi(input: *mut u8) -> *mut u8 {
                    use ::sbor::{Describe, Type};
                    use ::scrypto::abi::{BlueprintAbi, Fn};
                    use ::sbor::rust::borrow::ToOwned;
                    use ::sbor::rust::vec;
                    use ::sbor::rust::vec::Vec;
                    let fns: Vec<Fn> = vec![
                        ::scrypto::abi::Fn {
                            ident: "x".to_owned(),
                            mutability: Option::Some(::scrypto::abi::SelfMutability::Immutable),
                            input: Test_x_Input::describe(),
                            output: <u32>::describe(),
                            export_name: "Test_x".to_string(),
                        },
                        ::scrypto::abi::Fn {
                            ident: "y".to_owned(),
                            mutability: Option::None,
                            input: Test_y_Input::describe(),
                            output: <u32>::describe(),
                            export_name: "Test_y".to_string(),
                        }
                    ];
                    let structure: Type = Test_impl::Test::describe();
                    let output = BlueprintAbi {
                        structure,
                        fns,
                    };
                    ::scrypto::buffer::scrypto_encode_to_buffer(&output)
                }

                #[allow(non_camel_case_types)]
                #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
                pub struct TestComponent {
                    pub component: ::scrypto::component::Component,
                }

                impl ::scrypto::component::LocalComponent for TestComponent {
                    fn package_address(&self) -> PackageAddress {
                        self.component.package_address()
                    }
                    fn blueprint_name(&self) -> String {
                        self.component.blueprint_name()
                    }
                    fn add_access_check(&mut self, access_rules: ::scrypto::resource::AccessRules) -> &mut Self {
                        self.component.add_access_check(access_rules);
                        self
                    }
                    fn globalize(self) -> ComponentAddress {
                        self.component.globalize()
                    }
                }

                impl TestComponent {
                    pub fn y(arg0: u32) -> u32 {
                        ::scrypto::core::Runtime::call_function(::scrypto::core::Runtime::package_address(), "Test", "y", ::scrypto::args!(arg0))
                    }
                    pub fn x(&self, arg0: u32) -> u32 {
                        self.component.call("x", ::scrypto::args!(arg0))
                    }
                }
            },
        );
    }

    #[test]
    fn test_empty_blueprint() {
        let input = TokenStream::from_str("struct Test {} impl Test {}").unwrap();
        let output = handle_blueprint(input).unwrap();

        assert_code_eq(
            output,
            quote! {
                #[allow(non_snake_case)]
                pub mod Test_impl {
                    use super::*;

                    #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
                    pub struct Test {
                    }

                    impl Test {
                    }

                    impl ::scrypto::component::ComponentState<TestComponent> for Test {
                        fn instantiate(self) -> TestComponent {
                            let component = ::scrypto::component::component_system().create_component(
                                "Test",
                                self
                            );
                            TestComponent {
                                component
                            }
                        }
                    }
                }

                #[no_mangle]
                pub extern "C" fn Test_abi(input: *mut u8) -> *mut u8 {
                    use ::sbor::{Describe, Type};
                    use ::scrypto::abi::{BlueprintAbi, Fn};
                    use ::sbor::rust::borrow::ToOwned;
                    use ::sbor::rust::vec;
                    use ::sbor::rust::vec::Vec;
                    let fns: Vec<Fn> = vec![];
                    let structure: Type = Test_impl::Test::describe();
                    let output = BlueprintAbi {
                        structure,
                        fns,
                    };
                    ::scrypto::buffer::scrypto_encode_to_buffer(&output)
                }

                #[allow(non_camel_case_types)]
                #[derive(::sbor::TypeId, ::sbor::Encode, ::sbor::Decode, ::sbor::Describe)]
                pub struct TestComponent {
                    pub component: ::scrypto::component::Component,
                }

                impl ::scrypto::component::LocalComponent for TestComponent {
                    fn package_address(&self) -> PackageAddress {
                        self.component.package_address()
                    }
                    fn blueprint_name(&self) -> String {
                        self.component.blueprint_name()
                    }
                    fn add_access_check(&mut self, access_rules: ::scrypto::resource::AccessRules) -> &mut Self {
                        self.component.add_access_check(access_rules);
                        self
                    }
                    fn globalize(self) -> ComponentAddress {
                        self.component.globalize()
                    }
                }

                impl TestComponent {
                }
            },
        );
    }
}
