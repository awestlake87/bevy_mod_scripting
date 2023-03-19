extern crate bevy;

pub mod error;
#[cfg(feature = "lua")]
pub mod lua;
#[cfg(feature = "rhai")]
pub mod rhai;

pub mod common;

pub mod script_ref;
pub mod sub_reflect;
pub mod wrappers;

pub use {script_ref::*, sub_reflect::*};

pub mod prelude {
    #[cfg(feature = "lua")]
    pub use crate::{
        impl_lua_newtype,
        lua::{
            bevy::LuaBevyAPIProvider, std::LuaVec, FromLuaProxy, LuaProxyable, ReflectLuaProxyable,
            ToLuaProxy,
        },
    };

    #[cfg(feature = "rhai")]
    pub use crate::rhai::{
        bevy::RhaiBevyAPIProvider,
        std::{RhaiCopy, RhaiVec},
        FromRhaiProxy, ReflectRhaiProxyable, RhaiProxyable, ToRhaiProxy,
    };

    pub use crate::{common::bevy::GetWorld, impl_script_newtype, ValueIndex};
}

// re-export derive macros from other langs
pub use bevy_mod_scripting_derive::impl_script_newtype;
#[cfg(feature = "lua")]
pub use bevy_mod_scripting_lua_derive::impl_lua_newtype; //LuaProxy};

pub(crate) mod generated;

pub use parking_lot;

pub mod generator_prelude {
    #[cfg(feature = "lua")]
    pub use lua::*;
    #[cfg(feature = "lua")]
    mod lua {
        pub use crate::{
            error::ReflectionError,
            script_ref::{ReflectedValue, ValueIndex},
            sub_reflect::ReflectPathElem,
        };
        pub use bevy::prelude::App;
        pub use bevy::reflect::Enum;
        pub use bevy_mod_scripting_core::prelude::*;
        pub use bevy_mod_scripting_derive::impl_script_newtype;
        pub use std::ops::*;
        pub use std::sync::Mutex;
        pub use {
            crate::{common::bevy::GetWorld, lua::RegisterForeignLuaType},
            bevy_mod_scripting_lua::{docs::LuaDocFragment, tealr::mlu::mlua::MetaMethod},
            bevy_mod_scripting_lua_derive::impl_lua_newtype,
        };

        pub use crate::lua;
        pub use bevy_mod_scripting_core;
        pub use bevy_mod_scripting_lua;
    }
}
