use rustdoc_types::Type;

pub mod arg_validator;
pub mod config;
pub mod cratepath;
pub mod wrapper;
pub mod writer;

pub use {arg_validator::*, config::*, wrapper::*, writer::*};

use cratepath::{get_path, path_to_import};
use indexmap::{IndexMap, IndexSet};
use rustdoc_types::{Crate, Impl, Item, ItemEnum};
use serde_json::from_reader;
use std::{
    borrow::Cow,
    collections::HashSet,
    fs::File,
    io::{self, BufReader, Write},
};

/// Currently only used for stringifying simple trait names
pub fn stringify_type(type_: &Type) -> Option<String> {
    match type_ {
        Type::ResolvedPath(path) => Some(path.name.to_owned()),
        Type::Generic(s) | Type::Primitive(s) => Some(s.to_owned()),
        Type::QualifiedPath {
            name,
            args: _,
            self_type: _,
            trait_: _,
        } => Some(name.to_owned()),
        _ => None,
    }
}

pub(crate) fn write_use_items_from_path(
    module_name: &str,
    path_components: &[String],
    import_path: &String,
    out: &mut impl Write,
) -> io::Result<()> {
    // generate imports for each item
    write!(out, "use ")?;

    if !import_path.is_empty() {
        write!(out, "{}", &import_path)?;
    } else {
        if module_name.starts_with("bevy") && module_name.len() > 5 {
            write!(out, "bevy::")?;
            write!(out, "{}", &module_name[5..])?;
        } else {
            write!(out, "{}", module_name)?;
        }

        for item in path_components {
            write!(out, "::")?;
            write!(out, "{}", item)?;
        }
    }
    writeln!(out, ";")?;

    Ok(())
}

pub(crate) fn generate_cfg_feature_attribute(
    config: &Config,
    out: &mut impl Write,
) -> io::Result<()> {
    if config.required_features.len() == 1 {
        writeln!(out, "#[cfg(feature=\"{}\")]", config.required_features[0])?;
    } else if !config.required_features.is_empty() {
        writeln!(out, "#[cfg(all(")?;

        for feature in &config.required_features {
            writeln!(out, "feature=\"{}\",", feature)?;
        }

        writeln!(out, "))]")?;
    }

    Ok(())
}

pub(crate) fn generate_on_feature_attribute(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "#[languages(on_feature(lua))]")?;
    Ok(())
}

pub(crate) fn generate_macros(crates: &[Crate], config: Config, args: &Args) -> io::Result<()> {
    // the items we want to generate macro instantiations for
    let mut unmatched_types: HashSet<&String> = config.types.iter().map(|(k, _v)| k).collect();

    let mut wrapped_items: Vec<_> = crates
        .iter()
        .flat_map(|source| {
            source
                .index
                .iter()
                .filter(|(_id, item)| {
                    item.name
                        .as_ref()
                        .and_then(|k| config.types.get(k))
                        .map(|k| k.matches_result(item, source))
                        .unwrap_or(false)
                })
                .map(|(id, item)| {
                    // extract all available associated constants,methods etc available to this item
                    let mut self_impl: Option<&Impl> = None;
                    let mut impl_items: IndexMap<&str, Vec<(&Impl, &Item)>> = Default::default();
                    let mut implemented_traits: IndexSet<String> = Default::default();

                    let impls = match &item.inner {
                        ItemEnum::Struct(s) => &s.impls,
                        ItemEnum::Enum(e) => &e.impls,
                        _ => panic!("Only structs or enums are allowed!"),
                    };

                    impls.iter().for_each(|id| {
                        if let ItemEnum::Impl(i) = &source.index.get(id).unwrap().inner {
                            match &i.trait_ {
                                Some(t) => {
                                    implemented_traits.insert(t.name.to_owned());
                                }
                                None => self_impl = Some(i),
                            }
                            i.items.iter().for_each(|id| {
                                let it = source.index.get(id).unwrap();

                                impl_items
                                    .entry(it.name.as_ref().unwrap().as_str())
                                    .or_default()
                                    .push((i, it));
                            })
                        } else {
                            panic!("Expected impl items here!")
                        }
                    });

                    let config = config.types.get(item.name.as_ref().unwrap()).unwrap();

                    //let path_components = &source.paths.get(id).unwrap().path;
                    let path_components = get_path(id, source).unwrap_or_else(|| {
                        panic!("path not found for {:?} in {:?}", id, source.root)
                    });
                    //eprintln!("{:?}", path_components);
                    let path_components = path_to_import(path_components, source);
                    //eprintln!("{:?}", path_components);

                    let wrapper_name = format!("{WRAPPER_PREFIX}{}", item.name.as_ref().unwrap());
                    let wrapped_type = item.name.as_ref().unwrap();
                    WrappedItem {
                        wrapper_name,
                        wrapped_type,
                        path_components: Cow::Owned(path_components),
                        source,
                        config,
                        item,
                        self_impl,
                        impl_items,
                        crates,
                        has_global_methods: false,
                        implemented_traits,
                    }
                })
        })
        .collect();

    wrapped_items.iter().for_each(|v| {
        unmatched_types.remove(&v.wrapped_type);
    });

    if !unmatched_types.is_empty() {
        panic!("Some types were not found in the given crates: {unmatched_types:#?}")
    }

    let mut out = File::create(&config.output_file)?;

    // we want to preserve the original ordering from the config file
    wrapped_items.sort_by_cached_key(|f| config.types.get_index_of(f.wrapped_type).unwrap());

    writeln!(out, "#![allow(clippy::all,unused_imports)]")?;

    // user defined
    for import in config.imports.lines() {
        writeln!(out, "{}", import)?;
    }
    // automatic

    wrapped_items.iter().try_for_each(|item| {
        write_use_items_from_path(
            &item.config.source.0,
            &item.path_components[1..],
            &item.config.import_path,
            &mut out,
        )
    })?;

    let mut imported = HashSet::<String>::default();

    wrapped_items.iter().try_for_each(|item| {
        item.config
            .traits
            .iter()
            .try_for_each(|trait_methods| -> io::Result<()> {
                if !imported.contains(&trait_methods.name) {
                    write!(out, "use ")?;
                    write!(out, "{}", &trait_methods.import_path)?;
                    write!(out, ";")?;
                    writeln!(out)?;
                    imported.insert(trait_methods.name.to_owned());
                }

                Ok(())
            })
    })?;

    // make macro calls for each wrapped item
    wrapped_items
        .iter_mut()
        .try_for_each(|v| -> io::Result<()> {
            // macro invocation
            write!(out, "impl_script_newtype!")?;
            write!(out, "{{")?;

            generate_on_feature_attribute(&mut out)?;
            writeln!(out, "#[languages(on_feature(lua))]")?;

            v.write_type_docstring(&mut out, args)?;

            v.write_inline_full_path(&mut out, args)?;
            write!(out, " : ")?;
            writeln!(out)?;

            v.write_derive_flags_body(&config, &mut out, args)?;

            writeln!(out, "lua impl")?;
            write!(out, "{{")?;
            v.write_impl_block_body(&mut out, args)?;
            write!(out, "}}")?;

            write!(out, "}}")?;

            Ok(())
        })?;

    // write other code
    for line in config.other.lines() {
        writeln!(out, "{}", line)?;
    }

    // now create the API Provider
    // first the globals
    generate_cfg_feature_attribute(&config, &mut out)?;
    writeln!(out, "#[derive(Default)]")?;
    writeln!(out, "pub(crate) struct {}Globals;", config.api_name)?;

    generate_cfg_feature_attribute(&config, &mut out)?;
    write!(
        out,
        "impl bevy_mod_scripting_lua::tealr::mlu::ExportInstances for {}Globals",
        config.api_name
    )?;
    write!(out, "{{")?;
    writeln!(out, "fn add_instances<'lua, T: bevy_mod_scripting_lua::tealr::mlu::InstanceCollector<'lua>>(self, instances: &mut T) -> bevy_mod_scripting_lua::tealr::mlu::mlua::Result<()>")?;
    write!(out, "{{")?;
    for (global_name, type_, dummy_proxy) in wrapped_items
        .iter()
        .filter_map(|i| {
            i.has_global_methods.then_some((
                i.wrapped_type.as_str(),
                i.wrapper_name.as_str(),
                false,
            ))
        })
        .chain(config.manual_lua_types.iter().filter_map(|i| {
            i.include_global_proxy.then_some((
                i.proxy_name.as_str(),
                i.name.as_str(),
                i.use_dummy_proxy,
            ))
        }))
    {
        write!(out, "instances.add_instance(")?;
        // type name
        write!(out, "\"")?;
        write!(out, "{}", global_name)?;
        write!(out, "\"")?;
        // corresponding proxy
        if dummy_proxy {
            write!(out, ", crate::lua::util::DummyTypeName::<")?;
            write!(out, "{}", type_)?;
            write!(out, ">::new")?;
            write!(out, ")?;")?;
            writeln!(out)?;
        } else {
            write!(
                out,
                ", bevy_mod_scripting_lua::tealr::mlu::UserDataProxy::<"
            )?;
            write!(out, "{}", type_)?;
            write!(out, ">::new)?;")?;
            writeln!(out)?;
        }
    }

    writeln!(out, "Ok(())")?;
    write!(out, "}}")?;
    write!(out, "}}")?;

    // then the actual provider
    generate_cfg_feature_attribute(&config, &mut out)?;
    writeln!(out, "pub struct Lua{}Provider;", config.api_name)?;

    // begin impl {
    generate_cfg_feature_attribute(&config, &mut out)?;
    write!(out, "impl APIProvider for Lua{}Provider", config.api_name)?;
    write!(out, "{{")?;

    writeln!(
        out,
        "type APITarget = Mutex<bevy_mod_scripting_lua::tealr::mlu::mlua::Lua>;"
    )?;
    writeln!(
        out,
        "type ScriptContext = Mutex<bevy_mod_scripting_lua::tealr::mlu::mlua::Lua>;"
    )?;
    writeln!(out, "type DocTarget = LuaDocFragment;")?;

    // attach_api {
    write!(
        out,
        "fn attach_api(&mut self, ctx: &mut Self::APITarget) -> Result<(), ScriptError>",
    )?;
    write!(out, "{{")?;
    writeln!(
        out,
        "let ctx = ctx.get_mut().expect(\"Unable to acquire lock on Lua context\");"
    )?;
    writeln!(out, "bevy_mod_scripting_lua::tealr::mlu::set_global_env({}Globals,ctx).map_err(|e| ScriptError::Other(e.to_string()))", config.api_name)?;
    write!(out, "}}")?;
    // } attach_api

    // get_doc_fragment
    write!(out, "fn get_doc_fragment(&self) -> Option<Self::DocTarget>")?;
    write!(out, "{{")?;
    write!(
        out,
        "Some(LuaDocFragment::new(\"{}\", |tw|",
        config.api_name
    )?;
    write!(out, "{{")?;
    writeln!(out, "tw")?;
    writeln!(out, ".document_global_instance::<{}Globals>().expect(\"Something went wrong documenting globals\")", config.api_name)?;

    // include external types not generated by this file as well
    for (type_, include_proxy) in
        wrapped_items
            .iter()
            .map(|i| (i.wrapper_name.as_str(), i.has_global_methods))
            .chain(config.manual_lua_types.iter().filter_map(|i| {
                (!i.dont_process).then_some((i.name.as_str(), i.include_global_proxy))
            }))
    {
        write!(out, ".process_type::<")?;
        write!(out, "{}", type_)?;
        write!(out, ">()")?;
        writeln!(out)?;

        if include_proxy {
            write!(
                out,
                ".process_type::<bevy_mod_scripting_lua::tealr::mlu::UserDataProxy<",
            )?;
            write!(out, "{}", type_)?;
            write!(out, ">>()")?;
            writeln!(out)?;
        }
    }

    write!(out, "}}")?;
    writeln!(out, "))")?;

    write!(out, "}}")?;
    // } get_doc_fragment

    // impl default members
    for line in config.lua_api_defaults.lines() {
        writeln!(out, "{}", line)?;
    }

    // register_with_app {
    write!(out, "fn register_with_app(&self, app: &mut App)")?;
    write!(out, "{{")?;
    for item in wrapped_items
        .iter()
        .map(|i| i.wrapped_type)
        .chain(config.primitives.iter())
    {
        write!(out, "app.register_foreign_lua_type::<")?;
        write!(out, "{}", item)?;
        write!(out, ">();")?;
        writeln!(out)?;
    }
    write!(out, "}}")?;
    // } regiser_with_app

    write!(out, "}}")?;
    // } end impl

    Ok(())
}

pub fn generate_api_for_crates(args: &Args) -> Result<(), io::Error> {
    let crates: Vec<_> = args
        .json
        .iter()
        .map(|json| {
            let f = File::open(json).unwrap_or_else(|_| panic!("Could not open {}", json));
            let rdr = BufReader::new(f);
            from_reader(rdr)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut config = Config::load_from_toml_file(&args.config)?;

    config.types_.reverse();

    while !config.types_.is_empty() {
        let t = config.types_.remove(config.types_.len() - 1);
        config.types.insert(t.type_.to_string(), t);
    }

    generate_macros(&crates, config, args)?;

    Ok(())
}
