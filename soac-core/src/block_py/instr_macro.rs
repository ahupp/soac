#[macro_export]
macro_rules! define_instr {
    (
        $(#[$attrs:meta])*
        $vis:vis struct $name:ident<$expr_ty:ident> {
            $($fields:tt)*
        }
    ) => {
        define_instr!(
            @collect_fields
            [
                #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
                $(#[$attrs])*
            ]
            [$vis]
            [$name]
            [$expr_ty]
            [$($fields)*]
            []
            []
            []
            $($fields)*
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
    ) => {
        #[derive(Clone)]
        $($attrs)*
        $vis struct $name<$expr_ty: Instr> {
            _meta: Meta,
            pub extra: $expr_ty::Extra,
            $($struct_fields)*
        }

        impl<$expr_ty: Instr> std::fmt::Debug for $name<$expr_ty> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut debug = f.debug_tuple(stringify!($name));
                define_instr!(@debug_tuple_fields debug, self, $($raw_fields)*);
                debug.finish()
            }
        }

        impl<$expr_ty: Instr> $crate::block_py::PrettyPrint for $name<$expr_ty> {
            fn fmt_pretty(
                &self,
                printer: &mut $crate::block_py::PrettyPrinter<'_>,
            ) -> std::fmt::Result {
                std::fmt::Write::write_fmt(printer, format_args!("{self:?}"))
            }
        }

        impl<$expr_ty: Instr> $name<$expr_ty> {
            pub fn new($($ctor_args)*) -> Self {
                Self {
                    _meta: Meta::default(),
                    extra: Default::default(),
                    $($ctor_init)*
                }
            }

            pub fn with_extra(mut self, extra: $expr_ty::Extra) -> Self {
                self.extra = extra;
                self
            }

            pub fn extra(&self) -> &$expr_ty::Extra {
                &self.extra
            }

            pub fn extra_mut(&mut self) -> &mut $expr_ty::Extra {
                &mut self.extra
            }
        }

        impl<$expr_ty: Instr> HasMeta for $name<$expr_ty> {
            fn meta(&self) -> Meta {
                self._meta.clone()
            }
        }

        impl<$expr_ty: Instr> WithMeta for $name<$expr_ty> {
            fn with_meta(mut self, meta: Meta) -> Self {
                self._meta = meta;
                self
            }
        }

        impl<$expr_ty> ChildVisitable<$expr_ty> for $name<$expr_ty>
        where
            $expr_ty: Instr + ChildVisitable<$expr_ty>,
        {
            fn visit_children<V>(&self, visitor: &mut V)
            where
                V: $crate::block_py::Visit<$expr_ty> + ?Sized,
            {
                #[allow(unused_variables)]
                let _ = &visitor;
                define_instr!(@visit_expr_fields self, visitor, $($raw_fields)*);
            }

            fn visit_children_mut<V>(&mut self, visitor: &mut V)
            where
                V: $crate::block_py::VisitMut<$expr_ty> + ?Sized,
            {
                #[allow(unused_variables)]
                let _ = &visitor;
                define_instr!(@visit_expr_fields_mut self, visitor, $($raw_fields)*);
            }
        }

        impl<$expr_ty: Instr> Mappable<$expr_ty> for $name<$expr_ty> {
            type Mapped<T: Instr> = $name<T>;

            fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
            where
                T: Instr,
                M: MapInstr<$expr_ty, T>,
            {
                #[allow(unused_variables)]
                let _ = &map;
                define_instr!(@build_mapped [$name::<T>] [] self, map, $($raw_fields)*)
            }

            fn try_map_children<T, Error, M>(
                self,
                map: &mut M,
            ) -> Result<Self::Mapped<T>, Error>
            where
                T: Instr,
                M: TryMapInstr<$expr_ty, T, Error>,
            {
                #[allow(unused_variables)]
                let _ = &map;
                define_instr!(@build_try_mapped [$name::<T>] [] self, map, $($raw_fields)*)
            }

            fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<$expr_ty>
            where
                M: MapInstr<$expr_ty, $expr_ty>,
            {
                #[allow(unused_variables)]
                let _ = &map;
                define_instr!(@build_same_mapped [$name::<$expr_ty>] [] self, map, $($raw_fields)*)
            }

            fn try_map_same_children<Error, M>(
                self,
                map: &mut M,
            ) -> Result<Self::Mapped<$expr_ty>, Error>
            where
                M: TryMapInstr<$expr_ty, $expr_ty, Error>,
            {
                #[allow(unused_variables)]
                let _ = &map;
                define_instr!(@build_try_same_mapped [$name::<$expr_ty>] [] self, map, $($raw_fields)*)
            }
        }
    };
    (
        $(#[$attrs:meta])*
        $vis:vis struct $name:ident {
            $($fields:tt)*
        }
    ) => {
        define_instr!(
            @collect_value_fields
            [
                #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
                $(#[$attrs])*
            ]
            [$vis]
            [$name]
            []
            []
            []
            []
            $($fields)*
        );
    };
    (
        @collect_value_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
    ) => {
        #[derive(Clone)]
        $($attrs)*
        $vis struct $name {
            _meta: Meta,
            $($struct_fields)*
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut debug = f.debug_tuple(stringify!($name));
                define_instr!(@debug_tuple_fields debug, self, $($raw_fields)*);
                debug.finish()
            }
        }

        impl $crate::block_py::PrettyPrint for $name {
            fn fmt_pretty(
                &self,
                printer: &mut $crate::block_py::PrettyPrinter<'_>,
            ) -> std::fmt::Result {
                std::fmt::Write::write_fmt(printer, format_args!("{self:?}"))
            }
        }

        impl $name {
            pub fn new($($ctor_args)*) -> Self {
                Self {
                    _meta: Meta::default(),
                    $($ctor_init)*
                }
            }
        }

        impl HasMeta for $name {
            fn meta(&self) -> Meta {
                self._meta.clone()
            }
        }

        impl WithMeta for $name {
            fn with_meta(mut self, meta: Meta) -> Self {
                self._meta = meta;
                self
            }
        }

        impl<E: Instr> ChildVisitable<E> for $name {
            fn visit_children<V>(&self, visitor: &mut V)
            where
                V: $crate::block_py::Visit<E> + ?Sized,
            {
                let _ = &visitor;
            }

            fn visit_children_mut<V>(&mut self, visitor: &mut V)
            where
                V: $crate::block_py::VisitMut<E> + ?Sized,
            {
                let _ = &visitor;
            }
        }

        impl<E: Instr> Mappable<E> for $name {
            type Mapped<T: Instr> = $name;

            fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
            where
                T: Instr,
                M: MapInstr<E, T>,
            {
                let _ = &map;
                self
            }

            fn try_map_children<T, Error, M>(
                self,
                map: &mut M,
            ) -> Result<Self::Mapped<T>, Error>
            where
                T: Instr,
                M: TryMapInstr<E, T, Error>,
            {
                let _ = &map;
                Ok(self)
            }
        }
    };
    (
        @collect_value_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : $ty:ty,
        $($rest:tt)*
    ) => {
        define_instr!(
            @collect_value_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$($raw_fields)* $field: $ty,]
            [$($struct_fields)* pub $field: $ty,]
            [$($ctor_args)* $field: impl Into<$ty>,]
            [$($ctor_init)* $field: $field.into(),]
            $($rest)*
        );
    };
    (
        @collect_value_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : $ty:ty
    ) => {
        define_instr!(
            @collect_value_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$($raw_fields)* $field: $ty,]
            [$($struct_fields)* pub $field: $ty,]
            [$($ctor_args)* $field: impl Into<$ty>,]
            [$($ctor_init)* $field: $field.into(),]
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : Box<$inner_expr_ty:ident>,
        $($rest:tt)*
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: Box<$inner_expr_ty>,]
            [$($ctor_args)* $field: impl Into<Box<$inner_expr_ty>>,]
            [$($ctor_init)* $field: $field.into(),]
            $($rest)*
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : Vec<$inner_expr_ty:ident>,
        $($rest:tt)*
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: Vec<$inner_expr_ty>,]
            [$($ctor_args)* $field: impl Into<Vec<$inner_expr_ty>>,]
            [$($ctor_init)* $field: $field.into(),]
            $($rest)*
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : Vec<$inner_expr_ty:ident>
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: Vec<$inner_expr_ty>,]
            [$($ctor_args)* $field: impl Into<Vec<$inner_expr_ty>>,]
            [$($ctor_init)* $field: $field.into(),]
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : Option<Box<$inner_expr_ty:ident>>,
        $($rest:tt)*
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: Option<Box<$inner_expr_ty>>,]
            [$($ctor_args)* $field: impl Into<Option<Box<$inner_expr_ty>>>,]
            [$($ctor_init)* $field: $field.into(),]
            $($rest)*
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : Option<Box<$inner_expr_ty:ident>>
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: Option<Box<$inner_expr_ty>>,]
            [$($ctor_args)* $field: impl Into<Option<Box<$inner_expr_ty>>>,]
            [$($ctor_init)* $field: $field.into(),]
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : Box<$inner_expr_ty:ident>
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: Box<$inner_expr_ty>,]
            [$($ctor_args)* $field: impl Into<Box<$inner_expr_ty>>,]
            [$($ctor_init)* $field: $field.into(),]
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : $ty:ty,
        $($rest:tt)*
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: $ty,]
            [$($ctor_args)* $field: impl Into<$ty>,]
            [$($ctor_init)* $field: $field.into(),]
            $($rest)*
        );
    };
    (
        @collect_fields
        [$($attrs:tt)*]
        [$vis:vis]
        [$name:ident]
        [$expr_ty:ident]
        [$($raw_fields:tt)*]
        [$($struct_fields:tt)*]
        [$($ctor_args:tt)*]
        [$($ctor_init:tt)*]
        $field:ident : $ty:ty
    ) => {
        define_instr!(
            @collect_fields
            [$($attrs)*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($raw_fields)*]
            [$($struct_fields)* pub $field: $ty,]
            [$($ctor_args)* $field: impl Into<$ty>,]
            [$($ctor_init)* $field: $field.into(),]
        );
    };
    (@visit_expr_fields $self:ident, $visitor:ident,) => {};
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
        define_instr!(@visit_expr_fields $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Box<$expr_ty:ident>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
        define_instr!(@visit_expr_fields $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
        define_instr!(@visit_expr_fields $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
        define_instr!(@visit_expr_fields $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field(&$self.$field, $visitor);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        define_instr!(@visit_expr_fields $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : $ty:ty) => {};
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        for item in &$self.$field {
            $visitor.visit_instr(item);
        }
        define_instr!(@visit_expr_fields $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>) => {
        for item in &$self.$field {
            $visitor.visit_instr(item);
        }
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        if let Some(item) = &$self.$field {
            $visitor.visit_instr(item);
        }
        define_instr!(@visit_expr_fields $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        if let Some(item) = &$self.$field {
            $visitor.visit_instr(item);
        }
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident,) => {};
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
        define_instr!(@visit_expr_fields_mut $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Box<$expr_ty:ident>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
        define_instr!(@visit_expr_fields_mut $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
        define_instr!(@visit_expr_fields_mut $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>, $($rest:tt)*) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
        define_instr!(@visit_expr_fields_mut $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>) => {
        $crate::block_py::InstrField::<$expr_ty>::visit_field_mut(&mut $self.$field, $visitor);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        define_instr!(@visit_expr_fields_mut $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : $ty:ty) => {};
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        for item in &mut $self.$field {
            $visitor.visit_instr_mut(item);
        }
        define_instr!(@visit_expr_fields_mut $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Vec<$expr_ty:ident>) => {
        for item in &mut $self.$field {
            $visitor.visit_instr_mut(item);
        }
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        if let Some(item) = &mut $self.$field {
            $visitor.visit_instr_mut(item);
        }
        define_instr!(@visit_expr_fields_mut $self, $visitor, $($rest)*);
    };
    (@visit_expr_fields_mut $self:ident, $visitor:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        if let Some(item) = &mut $self.$field {
            $visitor.visit_instr_mut(item);
        }
    };
    (@debug_tuple_fields $builder:ident, $self:ident,) => {};
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        $builder.field(&$self.$field);
        define_instr!(@debug_tuple_fields $builder, $self, $($rest)*);
    };
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : Box<$expr_ty:ident>) => {
        $builder.field(&$self.$field);
    };
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        $builder.field(&$self.$field);
        define_instr!(@debug_tuple_fields $builder, $self, $($rest)*);
    };
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : $ty:ty) => {
        $builder.field(&$self.$field);
    };
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        $builder.field(&$self.$field);
        define_instr!(@debug_tuple_fields $builder, $self, $($rest)*);
    };
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : Vec<$expr_ty:ident>) => {
        $builder.field(&$self.$field);
    };
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        $builder.field(&$self.$field);
        define_instr!(@debug_tuple_fields $builder, $self, $($rest)*);
    };
    (@debug_tuple_fields $builder:ident, $self:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        $builder.field(&$self.$field);
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident,) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* }
    };
    (@build_walked [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident,) => {
        $($mapped_ctor)+ { _meta: $self._meta, $($out)* }
    };
    (@build_walked [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_walked
            [$($mapped_ctor)+]
            [$($out)* $field: Box::new($f(*$self.$field)),]
            $self,
            $f,
            $($rest)*
        )
    };
    (@build_walked [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : Box<$expr_ty:ident>) => {
        $($mapped_ctor)+ { _meta: $self._meta, $($out)* $field: Box::new($f(*$self.$field)), }
    };
    (@build_walked [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        define_instr!(
            @build_walked
            [$($mapped_ctor)+]
            [$($out)* $field: $self.$field,]
            $self,
            $f,
            $($rest)*
        )
    };
    (@build_walked [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty) => {
        $($mapped_ctor)+ { _meta: $self._meta, $($out)* $field: $self.$field, }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map), }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map), }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map), }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<T, M>($self.$field, $map), }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $map.map_name($self.$field),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $map.map_name($self.$field), }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $self.$field,]
            $self,
            $f,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $self.$field, }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: self.$field.into_iter().map(|value| $map.map_instr(value)).collect(),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: self.$field.into_iter().map(|value| $map.map_instr(value)).collect(), }
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: self.$field.map(|value| Box::new($map.map_instr(*value))),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $self.$field.map(|value| Box::new($map.map_instr(*value))), }
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident,) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?, })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?, })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?, })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<T, Error, M>($self.$field, $map)?, })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $map.try_map_name($self.$field)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $map.try_map_name($self.$field)?, })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $self.$field,]
            $self,
            $f,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $self.$field, })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $self.$field.into_iter().map(|value| $map.try_map_instr(value)).collect::<Result<Vec<_>, _>>()?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $self.$field.into_iter().map(|value| $map.try_map_instr(value)).collect::<Result<Vec<_>, _>>()?, })
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_try_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $self.$field.map(|value| $map.try_map_instr(*value).map(Box::new)).transpose()?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: Default::default(), $($out)* $field: $self.$field.map(|value| $map.try_map_instr(*value).map(Box::new)).transpose()?, })
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident,) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* }
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map), }
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map), }
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map), }
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::map_field::<$expr_ty, M>($self.$field, $map), }
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name, $($rest:tt)*) => {
        define_instr!(
            @build_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $map.map_name($self.$field),]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $map.map_name($self.$field), }
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        define_instr!(
            @build_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $self.$field,]
            $self,
            $f,
            $($rest)*
        )
    };
    (@build_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty) => {
        $($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $self.$field, }
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident,) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* })
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_try_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Box<$expr_ty:ident>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?, })
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>, $($rest:tt)*) => {
        define_instr!(
            @build_try_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$expr_ty:ident>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?, })
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_try_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Option<Box<$expr_ty:ident>>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?, })
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>, $($rest:tt)*) => {
        define_instr!(
            @build_try_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : Vec<$wrapper:ident<$expr_ty:ident>>) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $crate::block_py::InstrField::<$expr_ty>::try_map_field::<$expr_ty, Error, M>($self.$field, $map)?, })
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name, $($rest:tt)*) => {
        define_instr!(
            @build_try_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $map.try_map_name($self.$field)?,]
            $self,
            $map,
            $($rest)*
        )
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $map:ident, $field:ident : $expr_ty:ident::Name) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $map.try_map_name($self.$field)?, })
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty, $($rest:tt)*) => {
        define_instr!(
            @build_try_same_mapped
            [$($mapped_ctor)+]
            [$($out)* $field: $self.$field,]
            $self,
            $f,
            $($rest)*
        )
    };
    (@build_try_same_mapped [$($mapped_ctor:tt)+] [$($out:tt)*] $self:ident, $f:ident, $field:ident : $ty:ty) => {
        Ok($($mapped_ctor)+ { _meta: $self._meta, extra: $self.extra, $($out)* $field: $self.$field, })
    };
}

#[macro_export]
macro_rules! define_ruff_instr {
    (
        $(#[$attrs:meta])*
        $vis:vis struct $name:ident<$expr_ty:ident> {
            $($fields:tt)*
        }
    ) => {
        define_instr!(
            @collect_fields
            [$(#[$attrs])*]
            [$vis]
            [$name]
            [$expr_ty]
            [$($fields)*]
            []
            []
            []
            $($fields)*
        );
    };
    (
        $(#[$attrs:meta])*
        $vis:vis struct $name:ident {
            $($fields:tt)*
        }
    ) => {
        define_instr!(
            @collect_value_fields
            [$(#[$attrs])*]
            [$vis]
            [$name]
            []
            []
            []
            []
            $($fields)*
        );
    };
}

pub use crate::define_instr;
pub use crate::define_ruff_instr;
