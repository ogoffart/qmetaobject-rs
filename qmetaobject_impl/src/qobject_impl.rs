/* Copyright (C) 2018 Olivier Goffart <ogoffart@woboq.com>

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES
OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
*/
use super::qbjs;
use proc_macro::TokenStream;
use quote::ToTokens;
use std::iter::Iterator;
use syn;
use syn::parse::{Parse, ParseStream, Parser, Result};

macro_rules! unwrap_parse_error(
    ($e:expr) => {
        match $e {
            Ok(x) => x,
            Err(e) => { return e.to_compile_error().into() }
        }
    }
);

#[allow(non_snake_case)]
#[allow(non_upper_case_globals)]
#[allow(dead_code)]
mod MetaObjectCall {
    // QMetaObject::Call
    pub const InvokeMetaMethod: u32 = 0;
    pub const ReadProperty: u32 = 1;
    pub const WriteProperty: u32 = 2;
    pub const ResetProperty: u32 = 3;
    pub const QueryPropertyDesignable: u32 = 4;
    pub const QueryPropertyScriptable: u32 = 5;
    pub const QueryPropertyStored: u32 = 6;
    pub const QueryPropertyEditable: u32 = 7;
    pub const QueryPropertyUser: u32 = 8;
    pub const CreateInstance: u32 = 9;
    pub const IndexOfMethod: u32 = 10;
    pub const RegisterPropertyMetaType: u32 = 11;
    pub const RegisterMethodArgumentMetaType: u32 = 12;
}

fn builtin_type(ty: &syn::Type) -> u32 {
    if let syn::Type::Tuple(ref tuple) = ty {
        // Is it "()" aka the "void" type?
        return if tuple.elems.is_empty() { 43 } else { 0 };
    }
    match ty.clone().into_token_stream().to_string().as_ref() {
        "bool" => 1,
        "i32" => 2,
        "u32" => 3,
        "i64" => 4,
        "u64" => 5,
        "f64" => 6,
        "i16" => 33,
        "i8" => 34,
        "u16" => 36,
        "u8" => 37,
        "f32" => 38,
        //"*c_void" => 31,
        "QString" => 10,
        "QByteArray" => 12,
        "QVariant" => 41,
        _ => 0,
    }
}

trait IsVoid {
    fn is_void(&self) -> bool;
}

impl IsVoid for syn::Type {
    fn is_void(&self) -> bool {
        if let syn::Type::Tuple(tuple) = self {
            tuple.elems.is_empty()
        } else {
            false
        }
    }
}

fn write_u32(val: i32) -> [u8; 4] {
    [
        (val & 0xff) as u8,
        ((val >> 8) & 0xff) as u8,
        ((val >> 16) & 0xff) as u8,
        ((val >> 24) & 0xff) as u8,
    ]
}

#[derive(Clone)]
struct MetaMethodParameter {
    typ: syn::Type,
    name: Option<syn::Ident>,
}

#[derive(Clone)]
struct MetaMethod {
    name: syn::Ident,
    args: Vec<MetaMethodParameter>,
    // TODO: wrapper for `Qt::MethodFlags` and other enums
    /// Flags of `Qt::MethodFlags` enum.
    ///
    /// Enum members used in QObject generator are:
    ///  - `AccessPublic = 0x02`
    ///  - `MethodMethod = 0x00`
    ///  - `MethodSignal = 0x04`
    flags: u32,
    ret_type: syn::Type,
}

#[derive(Clone)]
struct MetaProperty {
    name: syn::Ident,
    typ: syn::Type,
    flags: u32,
    notify_signal: Option<syn::Ident>,
    getter: Option<syn::Ident>,
    setter: Option<syn::Ident>,
    alias: Option<syn::Ident>,
}

#[derive(Clone)]
struct MetaEnum {
    name: syn::Ident,
    variants: Vec<syn::Ident>,
}

#[derive(Default)]
struct MetaObject {
    int_data: Vec<proc_macro2::TokenStream>,
    string_data: Vec<String>,
}
impl MetaObject {
    fn build_string_data(&self, target_pointer_width: u32) -> Vec<u8> {
        let mut result: Vec<u8> = Vec::new();

        let sizeof_qbytearraydata = if target_pointer_width == 64 { 24 } else { 16 };
        let mut ofs = sizeof_qbytearraydata * self.string_data.len() as i32;
        for s in self.string_data.iter() {
            result.extend_from_slice(&write_u32(-1)); // ref (-1)
            result.extend_from_slice(&write_u32(s.len() as i32)); // size
            result.extend_from_slice(&write_u32(0)); // alloc / capacityReserved
            if target_pointer_width == 64 {
                result.extend_from_slice(&write_u32(0)); // padding
            }
            result.extend_from_slice(&write_u32(ofs)); // offset (LSB)
            if target_pointer_width == 64 {
                result.extend_from_slice(&write_u32(0)); // offset (MSB)
            }

            ofs += s.len() as i32 + 1; // +1 for the '\0'
            ofs -= sizeof_qbytearraydata;
        }

        for s in self.string_data.iter() {
            result.extend_from_slice(s.as_bytes());
            result.push(0); // null terminated
        }
        result
    }

    fn push_int(&mut self, i: u32) {
        self.int_data.push(quote!(#i));
    }

    fn extend_from_int_slice(&mut self, slice: &[u32]) {
        for i in slice {
            self.int_data.push(quote!(#i));
        }
    }

    fn compute_int_data(
        &mut self,
        class_name: String,
        properties: &[MetaProperty],
        methods: &[MetaMethod],
        enums: &[MetaEnum],
        signal_count: usize,
    ) {
        let has_notify = properties.iter().any(|p| p.notify_signal.is_some());
        self.add_string(class_name);
        self.add_string("".to_owned());

        let mut offset = 14;
        let property_offset = offset + methods.len() as u32 * 5;
        let enum_offset =
            property_offset + properties.len() as u32 * (if has_notify { 4 } else { 3 });

        self.extend_from_int_slice(&[
            7, // revision
            0, // classname
            0,
            0, // class info count and offset
            methods.len() as u32,
            if methods.is_empty() { 0 } else { offset }, // method count and offset
            properties.len() as u32,
            if properties.is_empty() { 0 } else { property_offset }, // properties count and offset
            enums.len() as u32,
            if enums.is_empty() { 0 } else { enum_offset }, // enum count and offset
            0,
            0,                   // constructor count and offset
            0x4,                 // flags (PropertyAccessInStaticMetaCall)
            signal_count as u32, // signalCount
        ]);

        offset = enum_offset + enums.len() as u32 * 4;

        for m in methods {
            let n = self.add_string(m.name.to_string());
            self.extend_from_int_slice(&[n, m.args.len() as u32, offset, 1, m.flags]);
            offset += 1 + 2 * m.args.len() as u32;
        }

        for p in properties {
            let n = self.add_string(p.alias.as_ref().unwrap_or(&p.name).to_string());
            let type_id = self.add_type(p.typ.clone());
            self.extend_from_int_slice(&[n, type_id, p.flags]);
        }

        for e in enums {
            let n = self.add_string(e.name.to_string());
            // name, flag, count, data offset
            self.extend_from_int_slice(&[n, 0x2, e.variants.len() as u32, offset]);
            offset += 2 * e.variants.len() as u32;
        }

        if has_notify {
            for p in properties {
                match p.notify_signal {
                    None => self.push_int(0 as u32),
                    Some(ref signal) => self.push_int(
                        methods
                            .iter()
                            .position(|x| x.name == *signal && (x.flags & 0x4) != 0)
                            .expect("Invalid NOTIFY signal") as u32,
                    ),
                };
            }
        }

        for m in methods {
            // return type
            let ret_type = self.add_type(m.ret_type.clone());
            self.push_int(ret_type);
            // types
            for a in m.args.iter() {
                let ty = self.add_type(a.typ.clone());
                self.push_int(ty);
            }
            // names
            for a in m.args.iter() {
                let n = self.add_string(a.name.clone().into_token_stream().to_string());
                self.push_int(n);
            }
        }

        for e in enums {
            for v in &e.variants {
                let n = self.add_string(v.to_string());
                // name, value
                self.push_int(n);
                let e_name = &e.name;
                self.int_data.push(quote! { #e_name::#v as u32 });
            }
        }
    }

    fn add_type(&mut self, ty: syn::Type) -> u32 {
        let mut type_id = builtin_type(&ty);
        let string = ty.into_token_stream().to_string();
        if type_id == 0 {
            type_id = self.add_string(string) | 0x80000000 /*IsUnresolvedType */;
        }
        type_id
    }

    fn add_string(&mut self, string: String) -> u32 {
        if let Some((pos, _)) = self.string_data.iter().enumerate().find(|(_, val)| *val == &string)
        {
            return pos as u32;
        }
        self.string_data.push(string);
        self.string_data.len() as u32 - 1
    }
}

fn map_method_parameters(
    args: &syn::punctuated::Punctuated<syn::FnArg, Token![,]>,
) -> Vec<MetaMethodParameter> {
    args.iter()
        .filter_map(|x| match x {
            syn::FnArg::Typed(cap) => Some(MetaMethodParameter {
                name: if let syn::Pat::Ident(ref id) = *cap.pat {
                    Some(id.ident.clone())
                } else {
                    None
                },
                typ: (*cap.ty).clone(),
            }),
            _ => None,
        })
        .collect()
}

fn map_method_parameters2(
    args: &syn::punctuated::Punctuated<syn::BareFnArg, Token![,]>,
) -> Vec<MetaMethodParameter> {
    args.iter()
        .filter_map(|x| {
            if let Some(ref name) = x.name {
                Some(MetaMethodParameter { name: Some(name.0.clone()), typ: x.ty.clone() })
            } else {
                None
            }
        })
        .collect()
}

pub fn generate(input: TokenStream, is_qobject: bool) -> TokenStream {
    let ast = parse_macro_input!(input as syn::DeriveInput);

    let name = &ast.ident;

    let mut properties = vec![];
    let mut methods = vec![];
    let mut signals = vec![];
    let mut func_bodies = vec![];
    let mut is_plugin = false;
    let mut plugin_iid: Option<syn::LitStr> = None;

    let crate_ = super::get_crate(&ast);
    let mut base: syn::Ident = parse_quote!(QGadget);
    let mut base_prop: syn::Ident = parse_quote!(missing_base_class_property);
    let mut has_base_property = false;

    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    if let syn::Data::Struct(ref data) = ast.data {
        for f in data.fields.iter() {
            use syn::Type::Macro;
            if let Macro(ref mac) = f.ty {
                if let Some(ref segment) = mac.mac.path.segments.last() {
                    match segment.ident.to_string().as_ref() {
                        "qt_property" => {
                            #[derive(Debug)]
                            enum Flag {
                                Notify(syn::Ident),
                                Read(syn::Ident),
                                Write(syn::Ident),
                                Alias(syn::Ident),
                                Const,
                            }
                            impl Parse for Flag {
                                fn parse(input: ParseStream) -> Result<Self> {
                                    let k = input.parse::<syn::Ident>()?;
                                    if &k == "NOTIFY" {
                                        Ok(Flag::Notify(input.parse()?))
                                    } else if &k == "CONST" {
                                        Ok(Flag::Const)
                                    } else if &k == "READ" {
                                        Ok(Flag::Read(input.parse()?))
                                    } else if &k == "WRITE" {
                                        Ok(Flag::Write(input.parse()?))
                                    } else if &k == "ALIAS" {
                                        Ok(Flag::Alias(input.parse()?))
                                    } else {
                                        Err(input.error("expected a property keyword"))
                                    }
                                }
                            }

                            let property_parser =
                                |input: ParseStream| -> Result<(syn::Type, Vec<Flag>)> {
                                    Ok((
                                        input.parse()?,
                                        input
                                            .parse::<Option<Token![;]>>()?
                                            .map(|_| -> Result<Vec<Flag>> {
                                                let mut r = Vec::<Flag>::new();
                                                while !input.is_empty() {
                                                    r.push(input.parse()?)
                                                }
                                                Ok(r)
                                            })
                                            .unwrap_or_else(|| Ok(Default::default()))?,
                                    ))
                                };

                            let parsed = unwrap_parse_error!(
                                property_parser.parse(mac.mac.tokens.clone().into())
                            );
                            let mut notify_signal = None;
                            let mut getter = None;
                            let mut setter = None;
                            let mut alias = None;
                            let mut flags = 1 | 2 | 0x00004000 | 0x00001000 | 0x00010000;
                            for it in parsed.1 {
                                match it {
                                    Flag::Notify(signal) => {
                                        assert!(
                                            notify_signal.is_none(),
                                            "Two NOTIFY for a property"
                                        );
                                        notify_signal = Some(signal);
                                        flags |= 0x00400000;
                                    }
                                    Flag::Const => {
                                        flags |= 0x00000400; // Constant
                                        flags &= !2; // Writable
                                    }
                                    Flag::Read(i) => {
                                        assert!(getter.is_none(), "Two READ for a property");
                                        getter = Some(i);
                                    }
                                    Flag::Write(i) => {
                                        assert!(setter.is_none(), "Two READ for a property");
                                        setter = Some(i);
                                    }
                                    Flag::Alias(i) => {
                                        assert!(alias.is_none(), "Two READ for a property");
                                        alias = Some(i);
                                    }
                                }
                            }
                            properties.push(MetaProperty {
                                name: f.ident.clone().expect("Property does not have a name"),
                                typ: parsed.0,
                                flags,
                                notify_signal,
                                getter,
                                setter,
                                alias,
                            });
                        }
                        "qt_method" => {
                            let name = f.ident.clone().expect("Method does not have a name");

                            let (output, args) = if let Ok(method_ast) =
                                syn::parse::<syn::ItemFn>(mac.mac.tokens.clone().into())
                            {
                                assert_eq!(method_ast.sig.ident, name);
                                let tts = &mac.mac.tokens;
                                func_bodies.push(quote! { #tts });
                                let args = map_method_parameters(&method_ast.sig.inputs);
                                (method_ast.sig.output, args)
                            } else if let Ok(method_decl) =
                                syn::parse::<syn::TypeBareFn>(mac.mac.tokens.clone().into())
                            {
                                let args = map_method_parameters2(&method_decl.inputs);
                                (method_decl.output, args)
                            } else {
                                panic!("Cannot parse qt_method {}", name);
                            };

                            let ret_type = match output {
                                syn::ReturnType::Default => parse_quote! {()},
                                syn::ReturnType::Type(_, ref typ) => (**typ).clone(),
                            };
                            methods.push(MetaMethod { name, args, flags: 0x2, ret_type });
                        }
                        "qt_signal" => {
                            let parser = syn::punctuated::Punctuated::<syn::FnArg, Token![,]>::parse_terminated;
                            let args_list =
                                unwrap_parse_error!(parser.parse(mac.mac.tokens.clone().into()));
                            let args = map_method_parameters(&args_list);
                            signals.push(MetaMethod {
                                name: f.ident.clone().expect("Signal does not have a name"),
                                args,
                                flags: 0x2 | 0x4,
                                ret_type: parse_quote! {()},
                            });
                        }
                        "qt_base_class" => {
                            let parser = |input: ParseStream| -> Result<syn::Ident> {
                                input.parse::<Token![trait]>()?;
                                input.parse()
                            };
                            base = unwrap_parse_error!(parser.parse(mac.mac.tokens.clone().into()));
                            base_prop = f.ident.clone().expect("base prop needs a name");
                            has_base_property = true;
                        }
                        "qt_plugin" => {
                            is_plugin = true;
                            let iid: syn::LitStr =
                                unwrap_parse_error!(syn::parse(mac.mac.tokens.clone().into()));
                            plugin_iid = Some(iid);
                        }
                        _ => {}
                    }
                }
            }
            for i in f.attrs.iter() {
                if let Ok(x) = i.parse_meta() {
                    if x.path().is_ident("qt_base_class") {
                        if let syn::Meta::NameValue(mnv) = x {
                            if let syn::Lit::Str(s) = mnv.lit {
                                base = unwrap_parse_error!(syn::parse_str(&s.value()));
                                base_prop = f.ident.clone().expect("base prop needs a name");
                                has_base_property = true;
                            } else {
                                panic!("Can't parse qt_base_class");
                            }
                        } else {
                            panic!("Can't parse qt_base_class");
                        }
                    }
                }
            }
        }
    } else {
        //Nope. This is an Enum. We cannot handle these!
        panic!("#[derive(QObject)] is only defined for structs, not for enums!");
    }

    if is_qobject && !has_base_property {
        panic!("#[derive(QObject)] needs at least one field of type qt_base_class!");
    }

    // prepend the methods in the signal
    let mut methods2 = signals.clone();
    methods2.extend(methods);
    let methods = methods2;

    let mut mo: MetaObject = Default::default();
    mo.compute_int_data(name.to_string(), &properties, &methods, &[], signals.len());

    let str_data32 = mo.build_string_data(32);
    let str_data64 = mo.build_string_data(64);
    let int_data = mo.int_data;

    use self::MetaObjectCall::*;

    let get_object = if is_qobject {
        quote! {
            let pinned = <#name #ty_generics as #crate_::QObject>::get_from_cpp(o);
            // FIXME: we should probably use borrow_mut here instead, but in a way which order re-entry
            #[allow(unused_variables)]
            let mut obj = &mut *pinned.as_ptr();

            assert_eq!(o, obj.get_cpp_object(), "Internal pointer invalid");

            struct Check<'check>(
                *mut ::std::os::raw::c_void,
                *const (#crate_::QObject + 'check)
            );

            impl<'check> ::std::ops::Drop for Check<'check> {
                fn drop(&mut self) {
                    assert_eq!(self.0, unsafe { &*self.1 }.get_cpp_object(),
                               "Internal pointer changed while borrowed");
                }
            }

            let _check = Check(o, obj as *const #crate_::QObject);
        }
    } else {
        quote! {
            #[allow(unused_variables)]
            let mut obj = ::std::mem::transmute::<*mut ::std::os::raw::c_void, &mut #name #ty_generics>(o);
        }
    };

    let property_meta_call: Vec<_> = properties
        .iter()
        .enumerate()
        .map(|(i, prop)| {
            let i = i as u32;
            let property_name = &prop.name;
            let typ = &prop.typ;

            let mut notify = quote!{};
            if let Some(ref signal) = prop.notify_signal {
                let args_count = methods.iter()
                    .find(|x| x.name == *signal && (x.flags & 0x4) != 0)
                    .map_or(0, |s| s.args.len());
                let signal: syn::Ident = signal.clone();
                notify = match args_count {
                    0 => quote!{ obj.#signal() },
                    1 => quote!{ obj.#signal(obj.#property_name.clone()) },
                    _ => panic!("NOTIFY signal {} for property {} has too many arguments",
                                signal, property_name),
                };
            }

            let getter = if let Some(ref getter) = prop.getter {
                let getter_ident: syn::Ident = getter.clone();
                quote!{
                    let mut tmp : #typ = obj.#getter_ident();
                    <#typ as #crate_::PropertyType>::pass_to_qt(&mut tmp, *a);
                }
            } else {
                quote!{ <#typ as #crate_::PropertyType>::pass_to_qt(&mut obj.#property_name, *a); }
            };

            let setter = if let Some(ref setter) = prop.setter {
                let setter_ident: syn::Ident = setter.clone();
                quote!{
                    obj.#setter_ident(<#typ as #crate_::PropertyType>::read_from_qt(*a));
                }
            } else {
                quote! {
                    obj.#property_name = <#typ as #crate_::PropertyType>::read_from_qt(*a);
                    #notify
                }
            };

            // register properties of non-built-in types: stringify type's TokenStream
            let register_type = if builtin_type(typ) == 0 {
                let typ_str = typ.clone().into_token_stream().to_string();
                let typ_str = typ_str.as_bytes();
                quote! { /* externally defined variables: register_result. */
                    // SAFETY: string generated from Rust type tokens should be a valid C string.
                    let name = unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(&[#(#typ_str ,)* 0u8]) };
                    *register_result = <#typ as #crate_::PropertyType>::register_type(name);
                }
            } else {
                quote!{}
            };

            quote! {
                #i => match c {
                    #ReadProperty => unsafe {
                        #get_object
                        #getter
                    },
                    #WriteProperty => unsafe {
                        #get_object
                        #setter
                    },
                    #ResetProperty => {/* TODO */},
                    #RegisterPropertyMetaType => {
                         // a[0]: registerResult, should set to id of registered type or -1 if a type
                         // could not be registered for any reason.
                         // SAFETY: Qt always passes a valid pointer here.
                        let register_result: &mut i32 = unsafe { &mut *((*a.offset(0)) as *mut i32) };
                        #register_type
                    },
                    _ => {}
                }
            }
        })
        .collect();

    let method_meta_call: Vec<_> = methods
        .iter()
        .enumerate()
        .map(|(i, method)| {
            let i = i as u32;
            let method_name: syn::Ident = method.name.clone();
            let args_call: Vec<_> = method
                .args
                .iter()
                .enumerate()
                .map(|(i, arg)| {
                    let i = i as isize;
                    let ty = &arg.typ;
                    quote! {
                        // a[1..=N] are pointers to the arguments
                        // References to the builtin types are reinterpreted as their Rust
                        // counterparts using `builtin_type()` mapping.
                        (*(*(a.offset(#i + 1)) as *const #ty)).clone()
                    }
                })
                .collect();

            let call = quote! { obj.#method_name(#(#args_call),*) };

            if method.ret_type.is_void() {
                quote! { #i => #call, }
            } else {
                let ret_type = &method.ret_type;
                quote! {
                    #i => {
                        let r = #call;
                        // a[0] is a pointer to the return value
                        // SAFETY: pointer to the return value is guaranteed to exist in array `a`,
                        // but it may be null if the caller is not interested in reading it.
                        let return_value = unsafe { (*a.offset(0)) as *mut #ret_type };
                        if let Some(return_ref) = unsafe { return_value.as_mut() } {
                            *return_ref = r;
                        }
                    }
                }
            }
        })
        .collect();

    let register_arguments: Vec<_> = methods
        .iter()
        .enumerate()
        .map(|(fn_i, method)| {
            let fn_i = fn_i as u32;
            let args: Vec<_> = method
                .args
                .iter()
                .enumerate()
                .map(|(arg_i, arg)| {
                    let arg_i = arg_i as u32;
                    let typ = &arg.typ;
                    // FIXME: there's minor code duplication with `property_meta_call`
                    if builtin_type(&typ) == 0 {
                        let typ_str = typ.clone().into_token_stream().to_string();
                        let typ_str = typ_str.as_bytes();
                        quote! { /* externally defined variables: arg_type, arg_index. */
                            #arg_i => {
                                // SAFETY: string generated from Rust type tokens should be a valid C string.
                                let name = unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(&[#(#typ_str ,)* 0u8]) };
                                let ty = <#typ as #crate_::QMetaType>::register(Some(name));
                                ty
                            }
                        }
                    } else {
                        quote!{}
                    }
                })
                .collect();

            quote! { /* externally defined variables: arg_type, arg_index. */
                #fn_i => {
                    match arg_index {
                        #(#args)*
                        _ => -1, // default when type is unknown
                    }
                }
            }
        })
        .collect();

    func_bodies.extend(signals.iter().enumerate().map(|(i, signal)| {
        let sig_name = &signal.name;
        let i = i as u32;
        let args_decl: Vec<_> = signal
            .args
            .iter()
            .map(|arg| {
                // FIXME!  we should probably use the signature verbatim
                let n = &arg.name;
                let ty = &arg.typ;
                quote! { #n : #ty }
            })
            .collect();
        let args_ptr: Vec<_> = signal
            .args
            .iter()
            .map(|arg| {
                let n = &arg.name;
                let ty = &arg.typ;
                quote! { unsafe { ::std::mem::transmute::<& #ty, *mut ::std::os::raw::c_void>(& #n) } }
            })
            .collect();
        let array_size = signal.args.len() + 1;
        quote! {
            #[allow(non_snake_case)]
            fn #sig_name(&self #(, #args_decl)*) {
                let a: [*mut ::std::os::raw::c_void; #array_size] = [ ::std::ptr::null_mut() #(, #args_ptr)* ];
                unsafe {
                    #crate_::invoke_signal(
                        (self as &#crate_::QObject).get_cpp_object(),
                        #name::static_meta_object(),
                        #i,
                        &a
                    )
                }
            }
        }
    }));

    // Despite its name, it actually handles signals.
    let index_of_method = signals.iter().enumerate().map(|(i, signal)| {
        let sig_name = &signal.name;
        // if signal == offset_of(signal field) then *result = index and return.
        quote! { /* externally defined variables: signal, result. */
            // SAFETY: no dereference of null pointer, only calculation of field's offset over
            // imaginary struct located at null. In Rust null is 0, thus aligned for any type.
            let offset = unsafe {
                let base = ::std::ptr::null() as *const #name #ty_generics;
                let field = (&(*base).#sig_name) as *const _ as usize;
                field - (base as usize)
            };
            if signal == offset  {
                *result = #i as i32;
                return;
            }
        }
    });

    let base_meta_object = if is_qobject {
        quote! { <#name #ty_generics as #base>::get_object_description().meta_object }
    } else {
        quote! { ::std::ptr::null() }
    };

    let mo = if ast.generics.params.is_empty() {
        quote! {
            qmetaobject_lazy_static! {
                static ref MO: #crate_::QMetaObject = #crate_::QMetaObject {
                    super_data: #base_meta_object,
                    string_data: STRING_DATA.as_ptr(),
                    data: INT_DATA.as_ptr(),
                    static_metacall: Some(static_metacall),
                    meta_types: ::std::ptr::null(),
                    extra_data: ::std::ptr::null(),
                };
            };
            return &*MO;
        }
    } else {
        let turbo_generics = ty_generics.as_turbofish();
        let (ty_generics, turbo_generics) = if ast.generics.type_params().count() != 0 {
            (quote!(#ty_generics), quote!(#turbo_generics))
        } else {
            (quote!(), quote!())
        };
        quote! {
            use ::std::sync::Mutex;
            use ::std::collections::HashMap;
            use ::std::any::TypeId;

            // FIXME! this could be global
            qmetaobject_lazy_static! {
                static ref HASHMAP: Mutex<HashMap<TypeId, Box<#crate_::QMetaObject>>> =
                    Mutex::new(HashMap::new());
            };
            let mut h = HASHMAP.lock().unwrap();
            let mo = h.entry(TypeId::of::<#name #ty_generics>()).or_insert_with(
                || Box::new(#crate_::QMetaObject {
                    super_data: #base_meta_object,
                    string_data: STRING_DATA.as_ptr(),
                    data: INT_DATA.as_ptr(),
                    static_metacall: Some(static_metacall #turbo_generics),
                    meta_types: ::std::ptr::null(),
                    extra_data: ::std::ptr::null(),
            }));
            return &**mo;
        }
    };

    let qobject_spec_func = if is_qobject {
        quote! {
            fn get_cpp_object(&self) -> *mut ::std::os::raw::c_void {
                self.#base_prop.get()
            }

            unsafe fn get_from_cpp<'pinned_ref>(
                ptr: *mut ::std::os::raw::c_void
            ) -> #crate_::QObjectPinned<'pinned_ref, Self>
            {
                let refcell_qobject: *const ::std::cell::RefCell<#crate_::QObject> = (<#name #ty_generics as #base>::get_object_description().get_rust_refcell)(ptr);
                // This is a bit ugly, but this is the only solution i found to downcast
                let refcell_type: &::std::cell::RefCell<#name #ty_generics> = ::std::mem::transmute::<_, (&::std::cell::RefCell<#name #ty_generics>, *const())>(refcell_qobject).0;
                return #crate_::QObjectPinned::new(refcell_type);
            }

            unsafe fn cpp_construct(
                pinned: &::std::cell::RefCell<Self>
            ) -> *mut ::std::os::raw::c_void
            {
                assert!(pinned.borrow().#base_prop.get().is_null());
                let object_ptr = #crate_::QObjectPinned::<#crate_::QObject>::new(pinned as &::std::cell::RefCell<#crate_::QObject>);
                let object_ptr_ptr : *const #crate_::QObjectPinned<#crate_::QObject> = &object_ptr;
                let rust_pinned = #crate_::QObjectPinned::<dyn #base>::new(pinned as &::std::cell::RefCell<dyn #base>);
                let rust_pinned_ptr : *const #crate_::QObjectPinned<dyn #base> = &rust_pinned;
                let n = (<#name #ty_generics as #base>::get_object_description().create)(
                    rust_pinned_ptr as *const ::std::os::raw::c_void,
                    object_ptr_ptr as *const ::std::os::raw::c_void,
                );
                pinned.borrow_mut().#base_prop.set(n);
                n
            }

            unsafe fn qml_construct(
                pinned: &::std::cell::RefCell<Self>,
                mem: *mut ::std::os::raw::c_void,
                extra_destruct: extern fn(*mut ::std::os::raw::c_void)
            ) {
                let object_ptr = #crate_::QObjectPinned::<#crate_::QObject>::new(pinned as &::std::cell::RefCell<#crate_::QObject>);
                let object_ptr_ptr : *const #crate_::QObjectPinned<#crate_::QObject> = &object_ptr;
                let rust_pinned = #crate_::QObjectPinned::<dyn #base>::new(pinned as &::std::cell::RefCell<dyn #base>);
                let rust_pinned_ptr : *const #crate_::QObjectPinned<dyn #base> = &rust_pinned;
                pinned.borrow_mut().#base_prop.set(mem);
                (<#name #ty_generics as #base>::get_object_description().qml_construct)(
                    mem,
                    rust_pinned_ptr as *const ::std::os::raw::c_void,
                    object_ptr_ptr as *const ::std::os::raw::c_void,
                    extra_destruct
                );
            }

            fn cpp_size() -> usize {
                <#name #ty_generics as #base>::get_object_description().size
            }
        }
    } else {
        quote! {}
    };

    let trait_name = if is_qobject {
        quote! { QObject }
    } else {
        quote! { QGadget }
    };


    let mut body = quote! {
        #[allow(non_snake_case)]
        impl #impl_generics #name #ty_generics #where_clause {
            #(#func_bodies)*
        }
        impl #impl_generics #crate_::#trait_name for #name #ty_generics #where_clause {
            fn meta_object(&self) -> *const #crate_::QMetaObject {
                Self::static_meta_object()
            }

            fn static_meta_object() -> *const #crate_::QMetaObject {

                #[cfg(target_pointer_width = "64")]
                static STRING_DATA : &'static [u8] = & [ #(#str_data64),* ];
                #[cfg(target_pointer_width = "32")]
                static STRING_DATA : &'static [u8] = & [ #(#str_data32),* ];

                static INT_DATA : &'static [u32] = & [ #(#int_data),* ];

                #[allow(unused_variables)]
                extern "C" fn static_metacall #impl_generics (
                    o: *mut ::std::os::raw::c_void,
                    c: u32,
                    idx: u32,
                    a: *const *mut ::std::os::raw::c_void
                ) #where_clause
                {
                    if c == #InvokeMetaMethod {
                        // a[0]: pointer to return value.
                        // a[1..=10]: pointers to arg0..arg9.
                        unsafe {
                            #get_object
                            match idx {
                                #(#method_meta_call)*
                                _ => {}
                            }
                        }
                    } else if c == #IndexOfMethod {
                        // a[0]: pointer to an index of the method, return value.
                        let result: &mut i32 = unsafe { &mut *((*a.offset(0)) as *mut i32) };
                        // a[1]: pointer to raw signal representation (in our case it's field's offset).
                        // SAFETY: Qt never passes us signal representations of other classes, so
                        // it is guaranteed that this representation was generated by our code,
                        // thus known to be safe.
                        let signal: usize = unsafe { *((*a.offset(1)) as *const usize) };
                        #(#index_of_method)*
                    } else if c == #RegisterMethodArgumentMetaType {
                        // a[0]: pointer to type of the argument at given index, return value.
                        // SAFETY: Qt always passes a valid pointer here, but it is 'out' parameter,
                        // thus may not be initialized and must not be read from.
                        let arg_type: &mut i32 = unsafe { &mut *((*a.offset(0)) as *mut i32) };
                        // a[1]: pointer to index of the requested argument.
                        // SAFETY: Qt always passes a valid pointer here and checks that index is >= 0.
                        let arg_index: u32 = unsafe { *((*a.offset(1)) as *const u32) };
                        *arg_type = match idx {
                            #(#register_arguments)*
                            _ => -1, // default when type is unknown
                        }
                    } else {
                        match idx {
                            #(#property_meta_call)*
                            _ => {}
                        }
                    }
                }
                #mo
            }

            #qobject_spec_func

        }

    };
    if is_plugin {
        use crate::qbjs::Value;
        let mut object_data: Vec<(&'static str, Value)> = vec![
            ("IID", Value::String(plugin_iid.unwrap().value())),
            ("className", Value::String(name.to_string())),
            ("version", Value::Double(f64::from(0x050100))),
            ("debug", Value::Double(0.0 as f64)),
            // ("MetaData"] = CDef->Plugin.MetaData;
        ];
        object_data.sort_by(|a, b| a.0.cmp(b.0));

        let plugin_data = qbjs::serialize(&object_data);
        let plugin_data_size = plugin_data.len();
        body = quote! {
            #body

            #[link_section = ".qtmetadata"]
            #[no_mangle]
            #[allow(non_upper_case_globals)]
            pub static qt_pluginMetaData: [u8 ; 20 + #plugin_data_size] = [
                b'Q', b'T', b'M', b'E', b'T', b'A', b'D', b'A', b'T', b'A', b' ', b' ',
                b'q', b'b', b'j', b's', 1, 0, 0, 0,
                #(#plugin_data),*
            ];

            #[no_mangle]
            pub extern fn qt_plugin_query_metadata() -> *const u8 {
                qt_pluginMetaData.as_ptr()
            }

            #[no_mangle]
            pub extern fn qt_plugin_instance() -> *mut ::std::os::raw::c_void {
                #crate_::into_leaked_cpp_ptr(#name::default())
            }

        }
    }
    body.into()
}

fn is_valid_repr_attribute(attribute: &syn::Attribute) -> bool {
    match attribute.parse_meta() {
        Ok(syn::Meta::List(list)) => {
            if list.path.is_ident("repr") && list.nested.len() == 1 {
                match &list.nested[0] {
                    syn::NestedMeta::Meta(syn::Meta::Path(word)) => {
                        const ACCEPTABLE_TYPES: &[&str] = &["u8", "u16", "u32", "i8", "i16", "i32"];
                        ACCEPTABLE_TYPES.iter().any(|w| word.is_ident(w))
                    }
                    _ => false,
                }
            } else {
                false
            }
        }
        _ => false,
    }
}

pub fn generate_enum(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as syn::DeriveInput);

    let name = &ast.ident;

    let mut is_repr_explicit = false;
    for attr in &ast.attrs {
        is_repr_explicit |= is_valid_repr_attribute(attr);
    }
    if !is_repr_explicit {
        panic!("#[derive(QEnum)] only support enum with explicit #[repr(*)], \
                possible integer type are u8, u16, u32, i8, i16, i32.")
    }

    let crate_ = super::get_crate(&ast);
    let mut meta_enum = MetaEnum { name: name.clone(), variants: Vec::new() };

    if let syn::Data::Enum(ref data) = ast.data {
        for variant in data.variants.iter() {
            match &variant.fields {
                syn::Fields::Unit => {}
                // TODO report error with span
                _ => panic!("#[derive(QEnum)] only support field-less enum"),
            }

            let var_name = &variant.ident;
            meta_enum.variants.push(var_name.clone());
        }
    } else {
        panic!("#[derive(QEnum)] is only defined for enums, not for structs!");
    }

    let enums = vec![meta_enum];
    let mut mo: MetaObject = Default::default();
    mo.compute_int_data(name.to_string(), &[], &[], &enums, 0);
    let str_data32 = mo.build_string_data(32);
    let str_data64 = mo.build_string_data(64);
    let int_data = mo.int_data;

    let mo = if ast.generics.params.is_empty() {
        quote! {
            qmetaobject_lazy_static! {
                static ref MO: #crate_::QMetaObject = #crate_::QMetaObject {
                    super_data: ::std::ptr::null(),
                    string_data: STRING_DATA.as_ptr(),
                    data: INT_DATA.as_ptr(),
                    static_metacall: None,
                    meta_types: ::std::ptr::null(),
                    extra_data: ::std::ptr::null(),
                };
            };
            return &*MO;
        }
    } else {
        panic!("#[derive(QEnum)] is only defined for C enums, doesn't support generics");
    };

    let body = quote! {
        impl #crate_::QEnum for #name {
            fn static_meta_object() -> *const #crate_::QMetaObject {

                #[cfg(target_pointer_width = "64")]
                static STRING_DATA : &'static [u8] = & [ #(#str_data64),* ];
                #[cfg(target_pointer_width = "32")]
                static STRING_DATA : &'static [u8] = & [ #(#str_data32),* ];

                static INT_DATA : &'static [u32] = & [ #(#int_data),* ];

                #mo
            }
        }
    };
    body.into()
}
