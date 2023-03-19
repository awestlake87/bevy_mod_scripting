use std::{
    borrow::Cow,
    collections::HashSet,
    io::{self, BufWriter, Write},
};

use indexmap::{IndexMap, IndexSet};
use rustdoc_types::{Crate, Id, Impl, Item, ItemEnum, StructKind};

use crate::{Arg, ArgType, ArgWrapperType, Args, Config, Newtype};

pub static WRAPPER_PREFIX: &str = "Lua";

#[derive(Debug)]
pub struct WrappedItem<'a> {
    pub wrapper_name: String,
    pub wrapped_type: &'a String,
    pub path_components: Cow<'a, [String]>,
    pub source: &'a Crate,
    pub config: &'a Newtype,
    pub item: &'a Item,
    /// The items coming from all trait implementations
    pub impl_items: IndexMap<&'a str, Vec<(&'a Impl, &'a Item)>>,
    pub implemented_traits: IndexSet<String>,
    pub self_impl: Option<&'a Impl>,
    pub crates: &'a [Crate],
    /// If this type has some things which are "static" this is set to true later
    pub has_global_methods: bool,
}

impl WrappedItem<'_> {
    /// Writes full type path inline corresponding to `Reflect::type_name` of each type
    ///
    /// As:
    /// ```rust,ignore
    ///
    /// this
    /// |
    /// |..........|
    /// my_type_path::Type : Value :
    ///  UnaryOps( ...
    /// ```
    pub fn write_inline_full_path(&self, out: &mut impl Write, _: &Args) -> Result<(), io::Error> {
        if self.config.import_path.is_empty() {
            write!(out, "{}", self.path_components.join("::"))?;
        } else {
            write!(out, "{}", self.config.import_path)?;
        }

        Ok(())
    }

    /// Writes the docstring for the type over multiple lines
    ///
    /// As:
    /// ```rust,ignore
    ///
    /// /// generated docstring
    /// /// here
    /// my_macro_key : Value :
    ///  UnaryOps(
    ///  ...
    ///  )
    ///  +
    ///  ...
    /// ```
    pub fn write_type_docstring(&self, out: &mut impl Write, _: &Args) -> Result<(), io::Error> {
        let strings = if let Some(d) = &self.config.doc {
            d.to_string()
        } else {
            self.item.docs.as_ref().cloned().unwrap_or_default()
        };
        for l in strings.lines() {
            writeln!(out, "/// {}", l)?;
        }

        Ok(())
    }

    /// Writes the docstring for the given auto method over multiple lines
    ///
    /// As:
    /// ```rust,ignore
    ///
    ///
    /// my_macro_key : Value :
    ///  Methods(
    ///        /// generated docstring
    ///        /// here
    ///        my_method(usize) -> u32
    ///  )
    ///  +
    ///  ...
    /// ```
    pub fn write_method_docstring(
        &self,
        id: &Id,
        out: &mut impl Write,
        _: &Args,
    ) -> io::Result<()> {
        self.source
            .index
            .get(id)
            .unwrap()
            .docs
            .as_ref()
            .cloned()
            .unwrap_or_else(|| "".to_owned())
            .lines()
            .try_for_each(|l| -> io::Result<()> {
                writeln!(out, "/// {}", l)?;
                Ok(())
            })
    }

    /// Writes the contents of the impl block for this wrapper
    ///
    /// As:
    ///
    /// ```rust,ignore
    ///     impl {
    ///     ... // this!
    ///     }
    /// ```
    pub fn write_impl_block_body(&self, out: &mut impl Write, _: &Args) -> io::Result<()> {
        self.config.lua_methods.iter().try_for_each(|v| {
            writeln!(out, "{};", v)?;
            Ok(())
        })
    }

    /// Generates all derive flags for the type,
    ///
    /// Returns additional imports necessary for the generated methods
    ///
    /// ```rust,ignore
    /// my_type::Type : Value:
    /// ... // flags go here
    /// ```
    pub fn write_derive_flags_body(
        &mut self,
        config: &Config,
        out: &mut impl Write,
        args: &Args,
    ) -> io::Result<()> {
        if self.implemented_traits.contains("Clone") {
            // this flag requires cloning
            writeln!(out, "Clone +")?;
        }

        if self.implemented_traits.contains("Debug") {
            // this flag requires cloning
            writeln!(out, "Debug +")?;
        }

        let mut used_method_identifiers: HashSet<&str> = HashSet::default();

        writeln!(out, "Methods")?;
        write!(out, "(")?;
        let mut has_global_methods = false;
        self.impl_items
            .iter()
            .flat_map(|(_, items)| items.iter())
            .try_for_each(|(impl_, v)| -> io::Result<()>{
                // only select trait methods are allowed
                if let Some(trait_) = &impl_.trait_ {
                    if self
                        .config
                        .traits
                        .iter()
                        .any(|f| {
                            trait_.name == f.name
                        })
                    {
                        // keep going
                    } else {
                        return Ok(());
                    }
                };

                let (decl, generics) = match &v.inner {
                    ItemEnum::Function(f) => (&f.decl, &f.generics),
                    _ => return Ok(()),
                };

                let mut errors = Vec::default();

                let mut inner_writer = BufWriter::new(vec![]);

                self.write_method_docstring(&v.id, &mut inner_writer, args)?;

                write!(inner_writer, "{}", v.name.as_ref().unwrap())?;
                write!(inner_writer, "(")?;
                let mut is_global_method = true;
                decl.inputs
                    .iter()
                    .enumerate()
                    .try_for_each(|(i, (declaration_name, tp))| -> io::Result<()> {
                        let arg_type: Result<ArgType, _> = tp.try_into();

                        if let Ok(arg_type) = arg_type {
                            // if the underlying ident is self, we shouldn't wrap it when printing it
                            // if type is unknown no wrapper exists
                            let wrapper_type: Option<ArgWrapperType> = ArgWrapperType::with_config(self.wrapped_type, &arg_type, config);

                            match wrapper_type {
                                Some(w) => {
                                    write!(inner_writer, "{}", Arg::new(arg_type, w))?;
                                }
                                None => {
                                    write!(inner_writer, "<invalid: {arg_type}>")?;
                                    errors.push(format!("Unsupported argument {}, not a wrapped type or primitive", arg_type));
                                    return Ok(());
                                }
                            };

                            if declaration_name != "self" && i + 1 != decl.inputs.len() {
                                write!(inner_writer, ",")?;
                            } else if declaration_name == "self" {
                                is_global_method = false;
                                // macro needs to recognize the self receiver
                                write!(inner_writer, ":")?;
                            }
                        } else {
                            errors.push(format!("Unsupported argument, Not a simple type: {}.", arg_type.unwrap_err()))
                        };

                        Ok(())
                    })?;

                if is_global_method {
                    has_global_methods = true;
                }

                write!(inner_writer, ")")?;

                if let Some(tp) = &decl.output{
                    let arg_type: Result<ArgType, _> = tp.try_into();
                    if let Ok(arg_type) = arg_type {
                        if let ArgType::Ref { .. } = arg_type {
                            errors.push("references are not supported as return types".to_owned());
                            return Ok(());
                        }

                        // if the underlying ident is self, we shouldn't wrap it when printing it
                        // if type is unknown, no wrapper type exists
                        let wrapper_type: Option<ArgWrapperType> = ArgWrapperType::with_config(self.wrapped_type, &arg_type, config);

                        match wrapper_type {
                            Some(w) => {
                                write!(inner_writer, " -> ")?;
                                write!(inner_writer, "{}", &Arg::new(arg_type, w))?;
                            }
                            None => {
                                errors.push(format!("Unsupported argument, not a wrapped type or primitive {arg_type}"));
                                write!(inner_writer, "<invalid: {arg_type}>")?;
                            }
                        }
                    } else {
                        errors.push(format!("Unsupported argument, not a simple type: {}", arg_type.unwrap_err()))
                    }
                };

                if !generics.params.is_empty() {
                    errors.push("Generics on the method".to_owned());
                }

                if !errors.is_empty() {
                    if args.print_errors {
                        writeln!(out, "// Exclusion reason: {}", errors.join(","))?;

                        let inner = String::from_utf8(inner_writer.into_inner().unwrap()).unwrap();
                        for line in inner.lines() {
                            writeln!(out, "// {}", line)?;
                        }
                        writeln!(out)?;
                    }
                } else {
                    used_method_identifiers.insert(v.name.as_deref().unwrap());
                    write!(inner_writer, ",")?;

                    let inner = String::from_utf8(inner_writer.into_inner().unwrap()).unwrap();
                    writeln!(out, "{}", inner)?;
                }

                Ok(())
            })?;

        self.has_global_methods = has_global_methods;
        write!(out, ")")?;

        writeln!(out, "+ Fields")?;
        write!(out, "(")?;

        if let ItemEnum::Struct(struct_) = &self.item.inner {
            if let StructKind::Plain {
                fields,
                fields_stripped: _,
            } = &struct_.kind
            {
                fields
                    .iter()
                    .map(|field_| self.source.index.get(field_).unwrap())
                    .filter_map(|field_| match &field_.inner {
                        ItemEnum::StructField(type_) => {
                            Some((field_.name.as_ref().unwrap(), type_, field_))
                        }
                        _ => None,
                    })
                    .filter_map(|(name, type_, field_)| {
                        let arg_type: ArgType = type_.try_into().ok()?;
                        let base_ident = arg_type
                            .base_ident() // resolve self
                            .unwrap_or(self.wrapped_type.as_str());

                        // if the underlying ident is self, we shouldn't wrap it when printing it
                        let wrapper: ArgWrapperType = arg_type
                            .is_self()
                            .then_some(ArgWrapperType::None)
                            .or_else(|| {
                                config
                                    .primitives
                                    .contains(base_ident)
                                    .then_some(ArgWrapperType::Raw)
                            })
                            .or_else(|| {
                                config
                                    .types
                                    .contains_key(base_ident)
                                    .then_some(ArgWrapperType::Wrapped)
                            })
                            // we allow this since we later resolve unknown types to be resolved as ReflectedValues
                            .unwrap_or(ArgWrapperType::None);

                        let arg = Arg::new(arg_type, wrapper);
                        let mut reflectable_type = arg.to_string();

                        // if we do not have an appropriate wrapper and this is not a primitive or it's not public
                        // we need to go back to the reflection API
                        if arg.wrapper == ArgWrapperType::None {
                            if field_.attrs.iter().any(|attr| attr == "#[reflect(ignore)]") {
                                return None;
                            }

                            reflectable_type = "Raw(ReflectedValue)".to_owned();
                        }

                        if let Some(docs) = &field_.docs {
                            docs.lines().for_each(|line| {
                                writeln!(out, "/// {}", line).unwrap();
                            });
                        };

                        // add underscore if a method with same name exists
                        used_method_identifiers
                            .contains(name.as_str())
                            .then(|| writeln!(out, "#[rename(\"_{name}\")]").unwrap());
                        write!(out, "{}", name).unwrap();
                        write!(out, ": ").unwrap();
                        write!(out, "{}", &reflectable_type).unwrap();
                        write!(out, ",").unwrap();
                        writeln!(out).unwrap();

                        Some(())
                    })
                    .for_each(drop);
            }
        };
        write!(out, ")")?;

        static BINARY_OPS: [(&str, &str); 5] = [
            ("add", "Add"),
            ("sub", "Sub"),
            ("div", "Div"),
            ("mul", "Mul"),
            ("rem", "Rem"),
        ];
        writeln!(out, "+ BinOps").unwrap();
        write!(out, "(").unwrap();
        BINARY_OPS.into_iter().for_each(|(op, rep)| {
            if let Some(items) = self.impl_items.get(op) {
                items
                    .iter()
                    .filter_map(|(impl_, item)| Some((impl_, item, (&impl_.for_).try_into().ok()?)))
                    .filter(|(_, _, self_type): &(&&Impl, &&Item, ArgType)| {
                        let base_ident =
                            self_type.base_ident().unwrap_or(self.wrapped_type.as_str());
                        let is_self_type_the_wrapper = (base_ident == self.wrapped_type)
                            && config.types.contains_key(base_ident);
                        let is_primitive = config.primitives.contains(base_ident);
                        is_self_type_the_wrapper || is_primitive
                    })
                    .for_each(|(impl_, item, _self_type)| {
                        let _ = match &item.inner {
                            ItemEnum::Function(m) => {
                                m.decl
                                    .inputs
                                    .iter()
                                    .map(|(_, t)| {
                                        // check arg is valid
                                        let arg_type: ArgType = t.try_into()?;

                                        // if the underlying ident is self, we shouldn't wrap it when printing it
                                        let wrapper_type = ArgWrapperType::with_config(
                                            self.wrapped_type,
                                            &arg_type,
                                            config,
                                        )
                                        .unwrap();

                                        Ok(Arg::new(arg_type, wrapper_type).to_string())
                                    })
                                    .collect::<Result<Vec<_>, _>>()
                                    .map(|v| v.join(&format!(" {} ", rep)))
                                    .and_then(|expr| {
                                        // then provide return type
                                        // for these traits that's on associated types within the impl
                                        let out_type = impl_
                                            .items
                                            .iter()
                                            .find_map(|v| {
                                                let item = self.source.index.get(v).unwrap();
                                                if let ItemEnum::AssocType { default, .. } =
                                                    &item.inner
                                                {
                                                    if let Some("Output") = item.name.as_deref() {
                                                        return Some(default.as_ref().unwrap());
                                                    }
                                                }
                                                None
                                            })
                                            .ok_or_else(|| expr.clone())?;

                                        let arg_type: ArgType = out_type.try_into()?;
                                        // if the underlying ident is self, we shouldn't wrap it when printing it
                                        let wrapper_type: ArgWrapperType =
                                            ArgWrapperType::with_config(
                                                self.wrapped_type,
                                                &arg_type,
                                                config,
                                            )
                                            .unwrap();

                                        if wrapper_type == ArgWrapperType::None {
                                            return Err(arg_type.to_string());
                                        }

                                        let return_string =
                                            Arg::new(arg_type, wrapper_type).to_string();

                                        write!(out, "{}", &expr).unwrap();
                                        write!(out, " -> ").unwrap();
                                        write!(out, "{}", &return_string).unwrap();
                                        write!(out, ",").unwrap();
                                        writeln!(out).unwrap();
                                        Ok(())
                                    })
                            }
                            _ => panic!("Expected method"),
                        };
                    })
            }
        });
        write!(out, ")")?;

        static UNARY_OPS: [(&str, &str); 1] = [("neg", "Neg")];

        writeln!(out, "+ UnaryOps")?;
        write!(out, "(")?;
        for (op, rep) in UNARY_OPS.into_iter() {
            if let Some(items) = self.impl_items.get(op) {
                for (_, _) in items.iter() {
                    writeln!(out, "{rep} self -> self")?;
                }
            }
        }
        write!(out, ")")?;

        for flag in self.config.derive_flags.iter() {
            write!(out, "+ ")?;
            for line in flag.lines() {
                writeln!(out, "{}", line)?;
            }
        }

        Ok(())
    }
}
