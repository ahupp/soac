//! Native frame roles for the deterministic generated-member producer.
use super::catalog::{StructType, text_is};
use super::edges::{CodeRole, Edge, Template};
use super::generation::{FieldRole, GeneratedRole, HashAction};
use super::invocation::Owner;
use super::native::{self, Frame};
use super::operands;
use super::produced::{self, methods};
use super::protocol::{
    Role, active_role, matches_class, matches_fields, matches_helper_frame, matches_parameters,
    plan, require,
};
use crate::strict_runtime_unavailable;
use pyo3::ffi;
use pyo3::prelude::*;

fn parameter(
    owner: &Owner<'_>,
    frame: Frame<'_>,
    role: Role,
    name: &str,
) -> PyResult<*mut ffi::PyObject> {
    frame.parameter(
        owner.owner().py(),
        owner.data().catalog.recipe(role.code().unwrap()),
        name,
    )
}

fn executing(
    owner: &Owner<'_>,
    frame: Frame<'_>,
    role: Role,
    name: &str,
) -> PyResult<*mut ffi::PyObject> {
    frame.executing(
        owner.owner().py(),
        owner.data().catalog.recipe(role.code().unwrap()),
        name,
    )
}

fn boolean(value: bool) -> *mut ffi::PyObject {
    unsafe {
        if value {
            ffi::Py_True()
        } else {
            ffi::Py_False()
        }
    }
}

#[derive(Clone, Copy)]
enum FieldSelection {
    Init,
    Positional,
    Keyword,
    Instance,
}

fn fields_match(
    owner: &Owner<'_>,
    actual: *mut ffi::PyObject,
    selection: FieldSelection,
) -> PyResult<bool> {
    let Some(length) = operands::sequence_len(actual) else {
        return Ok(false);
    };
    let mut position = 0;
    for (index, field) in plan(owner)?.generation.fields.iter().enumerate() {
        let selected = match selection {
            FieldSelection::Init => field.role != FieldRole::ClassVariable,
            FieldSelection::Positional => {
                field.role != FieldRole::ClassVariable && field.init && !field.kw_only
            }
            FieldSelection::Keyword => {
                field.role != FieldRole::ClassVariable && field.init && field.kw_only
            }
            FieldSelection::Instance => field.role == FieldRole::Instance,
        };
        if selected {
            if position >= length
                || operands::sequence_item(actual, position)
                    != operands::field(owner, index)?.as_ptr()
            {
                return Ok(false);
            }
            position += 1;
        }
    }
    Ok(position == length)
}

fn builder_matches(
    owner: &Owner<'_>,
    parent: Frame<'_>,
    parent_role: Role,
    actual: *mut ffi::PyObject,
) -> PyResult<bool> {
    Ok(
        actual == executing(owner, parent, parent_role, "func_builder")?
            && owner.data().catalog.matches_structure(
                owner.owner().py(),
                owner,
                StructType::Builder,
                actual,
            )?,
    )
}

pub(super) fn enter(
    owner: &Owner<'_>,
    parent: Frame<'_>,
    child: Frame<'_>,
    edge: Edge,
) -> PyResult<Role> {
    let py = owner.owner().py();
    let parent_role = active_role(owner, parent)?;
    require(
        owner,
        super::invocation::validate_catalog(owner)?
            && matches_fields(owner)?
            && matches_parameters(owner)?,
        "dataclass generated producer environment changed",
    )?;
    let generated = &plan(owner)?.generation;
    let selected = match edge {
        Edge::PrepareInit => Role::Init,
        Edge::PrepareField => Role::FieldInit,
        Edge::PrepareFrozen => Role::Frozen,
        Edge::PrepareHash => match generated.hash_action {
            HashAction::Generate => Role::HashAdd,
            HashAction::SetNone => Role::HashNone,
            _ => {
                return Err(strict_runtime_unavailable(
                    py,
                    "dataclass hash producer disagrees with the plan",
                ));
            }
        },
        Edge::PrepareRepr
        | Edge::PrepareEquality
        | Edge::PrepareOrdering
        | Edge::AddInit
        | Edge::AddFrozenSetattr
        | Edge::AddFrozenDelattr
        | Edge::AddHash => {
            let index = owner.data().code.get().unwrap().source_count.get();
            let fragment = generated
                .fragments
                .get(index)
                .ok_or_else(|| strict_runtime_unavailable(py, "extra dataclass source fragment"))?;
            let allowed = match edge {
                Edge::PrepareRepr => fragment.role == GeneratedRole::Repr,
                Edge::PrepareEquality => fragment.role == GeneratedRole::Equality,
                Edge::PrepareOrdering => matches!(
                    fragment.role,
                    GeneratedRole::Less
                        | GeneratedRole::LessEqual
                        | GeneratedRole::Greater
                        | GeneratedRole::GreaterEqual
                ),
                Edge::AddInit => fragment.role == GeneratedRole::Init,
                Edge::AddFrozenSetattr => fragment.role == GeneratedRole::FrozenSetattr,
                Edge::AddFrozenDelattr => fragment.role == GeneratedRole::FrozenDelattr,
                Edge::AddHash => fragment.role == GeneratedRole::Hash,
                _ => false,
            };
            require(
                owner,
                allowed
                    && operands::builder_request(owner, child, fragment, true)?
                    && builder_matches(
                        owner,
                        parent,
                        parent_role,
                        parameter(owner, child, Role::AddInit, "self")?,
                    )?,
                "dataclass builder source role or operands changed",
            )?;
            Role::add(fragment.role)
        }
        Edge::MakeAnnotations => Role::Annotate,
        Edge::InstallConditional => Role::SetGenerated,
        _ => {
            return Err(strict_runtime_unavailable(
                py,
                "dataclass generated edge has no selected role",
            ));
        }
    };
    require(
        owner,
        matches_helper_frame(owner, child, selected)?,
        "dataclass generated helper was replaced",
    )?;
    match selected {
        Role::Init => {
            let init = generated
                .fragment(GeneratedRole::Init)
                .ok_or_else(|| strict_runtime_unavailable(py, "initializer fragment is absent"))?;
            require(
                owner,
                fields_match(
                    owner,
                    parameter(owner, child, selected, "fields")?,
                    FieldSelection::Init,
                )? && fields_match(
                    owner,
                    parameter(owner, child, selected, "std_fields")?,
                    FieldSelection::Positional,
                )? && fields_match(
                    owner,
                    parameter(owner, child, selected, "kw_only_fields")?,
                    FieldSelection::Keyword,
                )? && parameter(owner, child, selected, "frozen")?
                    == boolean(owner.data().options.frozen)
                    && parameter(owner, child, selected, "slots")?
                        == boolean(owner.data().options.slots)
                    && parameter(owner, child, selected, "has_post_init")?
                        == boolean(generated.has_post_init)
                    && unsafe {
                        text_is(
                            parameter(owner, child, selected, "self_name")?,
                            &init.parameters[0],
                        )
                    }
                    && builder_matches(
                        owner,
                        parent,
                        parent_role,
                        parameter(owner, child, selected, "func_builder")?,
                    )?,
                "initializer producer arguments changed",
            )?;
        }
        Role::FieldInit => {
            let init = generated.fragment(GeneratedRole::Init).unwrap();
            require(
                owner,
                operands::field_index(owner, parameter(owner, child, selected, "f")?)?.is_some()
                    && parameter(owner, child, selected, "frozen")?
                        == boolean(owner.data().options.frozen)
                    && parameter(owner, child, selected, "slots")?
                        == boolean(owner.data().options.slots)
                    && unsafe {
                        text_is(
                            parameter(owner, child, selected, "self_name")?,
                            &init.parameters[0],
                        )
                    }
                    && parameter(owner, child, selected, "globals")?
                        == executing(owner, parent, parent_role, "locals")?,
                "initializer field producer arguments changed",
            )?;
        }
        Role::Frozen | Role::HashAdd => {
            require(
                owner,
                matches_class(owner, parameter(owner, child, selected, "cls")?)?
                    && fields_match(
                        owner,
                        parameter(owner, child, selected, "fields")?,
                        FieldSelection::Instance,
                    )?
                    && builder_matches(
                        owner,
                        parent,
                        parent_role,
                        parameter(owner, child, selected, "func_builder")?,
                    )?,
                "frozen/hash producer arguments changed",
            )?;
        }
        Role::HashNone => {
            require(
                owner,
                matches_class(owner, parameter(owner, child, selected, "cls")?)?,
                "hash metadata target changed",
            )?;
        }
        Role::Annotate => {
            let name = parameter(owner, child, selected, "method_name")?;
            let index = produced::fragment_index(owner, name)
                .ok_or_else(|| strict_runtime_unavailable(py, "annotation method is absent"))?;
            let fragment = &generated.fragments[index];
            let annotation_fields = parameter(owner, child, selected, "annotation_fields")?;
            require(
                owner,
                matches_class(owner, parameter(owner, child, selected, "__class__")?)?
                    && name == executing(owner, parent, parent_role, "name")?
                    && fragment.annotation_fields.as_ref().is_some_and(|fields| {
                        operands::text_sequence(
                            annotation_fields,
                            fields.iter().map(String::as_str),
                        )
                    })
                    && parameter(owner, child, selected, "return_type")?
                        == unsafe { ffi::Py_None() }
                    && super::method_values::function_matches(
                        owner,
                        index,
                        executing(owner, parent, parent_role, "fn")?,
                        false,
                    )?,
                "generated annotation producer changed",
            )?;
        }
        Role::SetGenerated => {
            let name = parameter(owner, child, selected, "name")?;
            let index = produced::fragment_index(owner, name)
                .ok_or_else(|| strict_runtime_unavailable(py, "generated member name is absent"))?;
            let function = parameter(owner, child, selected, "value")?;
            require(
                owner,
                matches_class(owner, parameter(owner, child, selected, "cls")?)?
                    && name == executing(owner, parent, parent_role, "name")?
                    && function == executing(owner, parent, parent_role, "fn")?
                    && super::method_values::function_matches(owner, index, function, false)?,
                "generated member installer changed",
            )?;
        }
        _ => {}
    }
    Ok(selected)
}

pub(super) fn enter_factory_child(
    owner: &Owner<'_>,
    parent: Frame<'_>,
    child: Frame<'_>,
) -> PyResult<Option<Role>> {
    let py = owner.owner().py();
    require(
        owner,
        operands::factory_values(owner, parent, false)? && matches_fields(owner)?,
        "generated factory captures changed",
    )?;
    let code = owner.data().code.get().unwrap();
    let Some(pair) = code.repr_calls.get() else {
        return Ok(None);
    };
    let Some(offset) = parent.instruction() else {
        return Ok(None);
    };
    if offset == pair[0] {
        require(
            owner,
            matches_helper_frame(owner, child, Role::RecursiveRepr)?
                && unsafe {
                    text_is(
                        parameter(owner, child, Role::RecursiveRepr, "fillvalue")?,
                        "...",
                    )
                },
            "repr decorator factory edge changed",
        )?;
        Ok(Some(Role::RecursiveRepr))
    } else if offset == pair[1] {
        let index = plan(owner)?
            .generation
            .fragments
            .iter()
            .position(|fragment| fragment.role == GeneratedRole::Repr)
            .ok_or_else(|| strict_runtime_unavailable(py, "repr fragment is absent"))?;
        require(
            owner,
            matches_helper_frame(owner, child, Role::ReprDecorator)?
                && super::method_values::function_matches(
                    owner,
                    index,
                    parameter(owner, child, Role::ReprDecorator, "user_function")?,
                    true,
                )?,
            "repr decorator application edge changed",
        )?;
        Ok(Some(Role::ReprDecorator))
    } else {
        Ok(None)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CreatedRole {
    ReprDecorator,
    Method { index: usize, implementation: bool },
    Annotation(usize),
}

impl CreatedRole {
    pub(super) fn native(self, owner: &Owner<'_>) -> PyResult<u32> {
        Ok(match self {
            Self::ReprDecorator => native::DECORATOR,
            Self::Annotation(_) => native::ANNOTATION_PROVIDER,
            Self::Method {
                implementation: true,
                ..
            } => native::REPR_IMPLEMENTATION,
            Self::Method {
                index,
                implementation: false,
            } => produced::native_role(plan(owner)?.generation.fragments[index].role),
        })
    }

    pub(super) fn birth<'a>(self, owner: &'a Owner<'_>) -> PyResult<&'a produced::Birth> {
        let methods = methods(owner)?;
        Ok(match self {
            Self::ReprDecorator => &methods.repr_decorator,
            Self::Annotation(index) => methods.methods[index].annotation.as_ref().unwrap(),
            Self::Method {
                index,
                implementation: true,
            } => methods.methods[index].implementation.as_ref().unwrap(),
            Self::Method {
                index,
                implementation: false,
            } => &methods.methods[index].function,
        })
    }
}

pub(super) fn creation(
    owner: &Owner<'_>,
    producer: Frame<'_>,
    code: *mut ffi::PyObject,
) -> PyResult<Option<CreatedRole>> {
    let py = owner.owner().py();
    let actual = active_role(owner, producer)?;
    let template = match actual {
        Role::RecursiveRepr => Template::ReprDecorator,
        Role::ReprDecorator => Template::ReprWrapper,
        Role::Annotate => Template::AnnotationProvider,
        Role::GeneratedFactory => {
            require(
                owner,
                operands::factory_values(owner, producer, false)? && matches_fields(owner)?,
                "generated factory operands changed at creation",
            )?;
            let tree = owner.reference(owner.data().generated_code)?;
            let index = owner
                .data()
                .code
                .get()
                .unwrap()
                .method_for_code(py, tree.as_ptr(), code)?
                .ok_or_else(|| {
                    strict_runtime_unavailable(py, "unplanned generated function code")
                })?;
            return Ok(Some(CreatedRole::Method {
                index,
                implementation: plan(owner)?.generation.fragments[index].role
                    == GeneratedRole::Repr,
            }));
        }
        _ => return Ok(None),
    };
    if !owner
        .data()
        .catalog
        .matches_code(py, owner, CodeRole::Template(template), code)?
    {
        return Ok(None);
    }
    match actual {
        Role::RecursiveRepr => {
            require(
                owner,
                unsafe { text_is(executing(owner, producer, actual, "fillvalue")?, "...") },
                "repr fill value changed",
            )?;
            Ok(Some(CreatedRole::ReprDecorator))
        }
        Role::ReprDecorator => {
            let index = plan(owner)?
                .generation
                .fragments
                .iter()
                .position(|fragment| fragment.role == GeneratedRole::Repr)
                .unwrap();
            let running = executing(owner, producer, actual, "repr_running")?;
            require(
                owner,
                !running.is_null()
                    && unsafe { ffi::PySet_CheckExact(running) } != 0
                    && unsafe { ffi::PySet_Size(running) } == 0
                    && super::method_values::function_matches(
                        owner,
                        index,
                        executing(owner, producer, actual, "user_function")?,
                        true,
                    )?,
                "repr implementation changed before wrapper creation",
            )?;
            Ok(Some(CreatedRole::Method {
                index,
                implementation: false,
            }))
        }
        Role::Annotate => {
            let index =
                produced::fragment_index(owner, executing(owner, producer, actual, "method_name")?)
                    .ok_or_else(|| {
                        strict_runtime_unavailable(py, "annotation method changed before creation")
                    })?;
            let fragment = &plan(owner)?.generation.fragments[index];
            let fields = executing(owner, producer, actual, "annotation_fields")?;
            require(
                owner,
                matches_class(owner, executing(owner, producer, actual, "__class__")?)?
                    && fragment.annotation_fields.as_ref().is_some_and(|expected| {
                        operands::text_sequence(fields, expected.iter().map(String::as_str))
                    })
                    && executing(owner, producer, actual, "return_type")?
                        == unsafe { ffi::Py_None() },
                "annotation captures changed before creation",
            )?;
            Ok(Some(CreatedRole::Annotation(index)))
        }
        _ => unreachable!(),
    }
}

pub(super) fn created(
    owner: &Owner<'_>,
    producer: Frame<'_>,
    function: *mut ffi::PyObject,
    role: u32,
) -> PyResult<bool> {
    let code = unsafe { (*function.cast::<ffi::PyFunctionObject>()).func_code };
    let Some(selected) = creation(owner, producer, code)? else {
        return Ok(false);
    };
    require(
        owner,
        selected.native(owner)? == role,
        "generated birth role changed",
    )?;
    let birth = selected.birth(owner)?;
    birth.claim(owner)?;
    birth.publish(owner, function)?;
    require(
        owner,
        creation(owner, producer, code)? == Some(selected)
            && birth.matches(owner, function, role)?,
        "generated function changed during weak publication",
    )?;
    Ok(true)
}

pub(super) fn source(
    owner: &Owner<'_>,
    parent: Frame<'_>,
    args: &[*mut ffi::PyObject],
) -> PyResult<()> {
    let py = owner.owner().py();
    let code = owner.data().code.get().unwrap();
    let index = code.source_count.get();
    let fragment = plan(owner)?
        .generation
        .fragments
        .get(index)
        .ok_or_else(|| strict_runtime_unavailable(py, "extra generated source event"))?;
    require(
        owner,
        active_role(owner, parent)? == Role::add(fragment.role)
            && args.len() == 1
            && unsafe { text_is(args[0], &fragment.source) }
            && operands::builder_request(owner, parent, fragment, false)?
            && matches_fields(owner)?,
        "generated source does not implement its selected role",
    )?;
    code.source_count.set(index + 1);
    Ok(())
}
