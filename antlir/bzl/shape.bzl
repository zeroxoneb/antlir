# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

# @lint-ignore-every BUCKRESTRICTEDSYNTAX

"""
shape.bzl provides a convenient strongly-typed bridge from Buck bzl parse
time to Python runtime.

## Shape objects
Shape objects are immutable instances of a shape type, that have been
validated to match the shape type spec as described below.

## Shape Types
Shape types are a collection of strongly typed fields that can be validated
at Buck parse time and at runtime (aka image build time).

## Field Types
A shape field is a named member of a shape type. There are a variety of field
types available:
  primitive types (bool, int, float, str)
  other shapes
  homogenous lists of a single `field` element type
  dicts with homogenous key `field` types and homogenous `field` value type
  enums with string values
  unions via shape.union(type1, type2, ...)

If using a union, use the most specific type first as Pydantic will attempt to
coerce to the types in the order listed
(see https://pydantic-docs.helpmanual.io/usage/types/#unions) for more info.

## Optional and Defaulted Fields
By default, fields are required to be set at instantiation time.

Fields declared with `shape.field(..., default='val')` do not have to be
instantiated explicitly.

Additionally, fields can be marked optional by using the `optional` kwarg in
`shape.field`.

For example, `shape.field(int, optional=True)` denotes an integer field that
may or may not be set in a shape object.

Obviously, optional fields are still subject to the same type validation as
non-optional fields, but only if they have a non-None value.

## Runtime Implementations
`shape.impl` codegens runtime parser/validation libraries in Rust and Python.
The `name` argument must match the name of the `*.shape.bzl` file without the
'.bzl' suffix.

`shape.impl` behaves like any other Buck target, and requires dependencies to be
explicitly set.

NOTE: `shape.bzl` can be used strictly for Buck-time safety without any runtime
library implementation, in which case a separated `.shape.bzl` file and
`shape.impl` targets are not required.

## Serialization formats
shape.bzl provides two mechanisms to pass shape objects to runtime code.

`shape.json_file` dumps a shape object to an output file. This can be read
from a file or resource, using `read_resource` or `read_file` of the
generated loader class.

`shape.python_data` dumps a shape object to a raw python source file. This
is useful for some cases where a python_binary is expected to be fully
self-contained, but still require some build-time information. It is also
useful in cases when shapes are being dynamically generated based on inputs
to a macro. See the docblock of the function for an example.

## Naming Conventions
Shape types should be named with a suffix of '_t' to denote that it is a
shape type.
Shape instances should conform to whatever convention is used where they are
declared (usually snake_case variables).

## Example usage

```
build_source_t = shape.shape(
    source=str,
    type=str,
)

mount_config_t = shape.shape(
    build_source = build_source_t,
    default_mountpoint=str,
    is_directory=bool,
)

mount_t = shape.shape(
    mount_config = mount_config_t,
    mountpoint = shape.field(str, optional=True),
    target = shape.field(str, optional=True),
)

mount = mount_t(
    mount_config=mount_config_t(
        build_source=build_source_t(
            source="/etc/fbwhoami",
            type="host",
        ),
        default_mountpoint="/etc/fbwhoami",
        is_directory=False,
    ),
)
```

See tests/shape_test.bzl for full example usage and selftests.
"""

load("@bazel_skylib//lib:shell.bzl", "shell")
load("@bazel_skylib//lib:types.bzl", "types")
load("//antlir/bzl:build_defs.bzl", "buck_genrule", "export_file", "python_library", "rust_library", "target_utils", "third_party")
load(":sha256.bzl", "sha256_b64")
load(":structs.bzl", "structs")
load(":target_helpers.bzl", "antlir_dep", "normalize_target")
load(":target_tagger_helper.bzl", "target_tagger_helper")

_NO_DEFAULT = struct(no_default = True)

_DEFAULT_VALUE = struct(__default_value_sentinel__ = True)

# Poor man's debug pretty-printing. Better version coming on a stack.
def _pretty(x):
    return structs.to_dict(x) if structs.is_struct(x) else x

# Returns True iff `instance` is a `shape.new(shape, ...)`.
def _is_instance(instance, shape):
    if _is_shape_constructor(shape):
        shape = shape(__internal_get_shape = True)
    if not _is_shape(shape):
        fail("Checking if {} is a shape instance, but {} is not a shape".format(
            _pretty(instance),
            _pretty(shape),
        ))
    return (
        structs.is_struct(instance) and
        getattr(instance, "__shape__", None) == shape
    )

def _get_is_instance_error(val, t):
    if not _is_instance(val, t):
        return (
            (
                "{} is not an instance of {} -- note that structs & dicts " +
                "are NOT currently automatically promoted to shape"
            ).format(
                _pretty(val),
                _pretty(t),
            ),
        )
    return None

def _check_type(x, t):
    """Check that x is an instance of t.
    This is a little more complicated than `isinstance(x, t)`, and supports
    more use cases. _check_type handles primitive types (bool, int, str),
    shapes and collections (dict, list).

    Return: None if successful, otherwise a str to be passed to `fail` at a
                site that has more context for the user
    """
    if t == int:
        if types.is_int(x):
            return None
        return "expected int, got {}".format(x)
    if t == bool:
        if types.is_bool(x):
            return None
        return "expected bool, got {}".format(x)
    if t == str:
        if types.is_string(x):
            return None
        return "expected str, got {}".format(x)
    if _is_enum(t):
        if x in t.enum:
            return None
        return "expected one of {}, got {}".format(t.enum, x)
    if t == _path:
        return _check_type(x, str)
    if hasattr(t, "__I_AM_TARGET__"):
        type_error = _check_type(x, str)
        if not type_error:
            if x.count(":") != 1:
                return "expected exactly one ':'"
            if x.count("//") > 1:
                return "expected at most one '//'"
            if x.startswith(":"):
                return None
            if x.count("//") != 1:
                return "expected to start with ':', or contain exactly one '//'"
            return None
        else:
            return type_error
    if _is_field(t):
        if t.optional and x == None:
            return None
        return _check_type(x, t.type)
    if _is_shape(t):
        # Don't need type-check the internals of `x` because we trust it to
        # have been type-checked at the time of construction.
        return _get_is_instance_error(x, t)
    if _is_collection(t):
        return _check_collection_type(x, t)
    if _is_union(t):
        _matched_type, error = _find_union_type(x, t)
        return error
    return "unsupported type {}".format(t)  # pragma: no cover

# Returns a mutually exclusive tuple:
#   ("matched type" or None, "error if no type matched" or None)
def _find_union_type(x, t):
    type_errors = []
    for union_t in t.union_types:
        type_error = _check_type(x, union_t)
        if type_error == None:
            return union_t, None
        type_errors.append(type_error)
    return None, "{} not matched in union {}: {}".format(
        x,
        t.union_types,
        "; ".join(type_errors),
    )

def _check_collection_type(x, t):
    if t.collection == dict:
        if not types.is_dict(x):
            return "{} is not dict".format(x)
        key_type, val_type = t.item_type
        for key, val in x.items():
            key_type_error = _check_type(key, key_type)
            if key_type_error:
                return "key: " + key_type_error
            val_type_error = _check_type(val, val_type)
            if val_type_error:
                return "val: " + val_type_error
        return None
    if t.collection == list:
        if not types.is_list(x) and not types.is_tuple(x):
            return "{} is not list".format(x)
        for i, val in enumerate(x):
            type_error = _check_type(val, t.item_type)
            if type_error:
                return "item {}: {}".format(i, type_error)
        return None
    return "unsupported collection type {}".format(t.collection)  # pragma: no cover

def _field(type, optional = False, default = _NO_DEFAULT):
    # there isn't a great reason to have a runtime language type be
    # `typing.Optional[T]` or `Option<T>`, while still having a default value,
    # and it makes code generation have more weird branches to keep track of, so
    # make that explicitly unsupported
    if optional and default != _NO_DEFAULT:
        fail("default_value must not be specified with optional")
    if optional:
        default = None

    type = _normalize_type(type)
    return struct(
        default = default,
        optional = optional,
        type = type,
    )

def _is_field(x):
    return structs.is_struct(x) and sorted(structs.to_dict(x).keys()) == sorted(["type", "optional", "default"])

def _dict(key_type, val_type, **field_kwargs):
    return _field(
        type = struct(
            collection = dict,
            item_type = (_normalize_type(key_type), _normalize_type(val_type)),
        ),
        **field_kwargs
    )

def _list(item_type, **field_kwargs):
    return _field(
        type = struct(
            collection = list,
            item_type = _normalize_type(item_type),
        ),
        **field_kwargs
    )

def _is_collection(x):
    return structs.is_struct(x) and sorted(structs.to_dict(x).keys()) == sorted(["collection", "item_type"])

def _is_union(x):
    return structs.is_struct(x) and sorted(structs.to_dict(x).keys()) == sorted(["union_types"])

def _union_type(*union_types):
    """
    Define a new union type that can be used when defining a field. Most
    useful when a union type is meant to be typedef'd and reused. To define
    a shape field directly, see shape.union.

    Example usage:
    ```
    mode_t = shape.union_t(int, str)  # could be 0o644 or "a+rw"

    type_a = shape.shape(mode=mode_t)
    type_b = shape.shape(mode=shape.field(mode_t, optional=True))
    ```
    """
    if len(union_types) == 0:
        fail("union must specify at least one type")
    return struct(
        union_types = tuple([_normalize_type(t) for t in union_types]),
    )

def _union(*union_types, **field_kwargs):
    return _field(
        type = _union_type(*union_types),
        **field_kwargs
    )

def _enum(*values, **field_kwargs):
    # since enum values go into class member names, they must be strings
    for val in values:
        if not types.is_string(val):
            fail("all enum values must be strings, got {}".format(_pretty(val)))
    return _field(
        type = struct(
            enum = tuple(values),
        ),
        **field_kwargs
    )

def _is_enum(t):
    return structs.is_struct(t) and sorted(structs.to_dict(t).keys()) == sorted(["enum"])

def _path(**field_kwargs):
    fail("shape.path() is no longer supported, use `shape.path` directly, or wrap in `shape.field()`")

def _shape(**fields):
    """
    Define a new shape type with the fields as given by the kwargs.

    Example usage:
    ```
    shape.shape(hello=str)
    ```
    """
    for name, f in fields.items():
        if name == "__I_AM_TARGET__":
            continue

        # Avoid colliding with `__shape__`. Also, in Python, `_name` is "private".
        if name.startswith("_"):
            fail("Shape field name {} must not start with _: {}".format(
                name,
                _pretty(fields),
            ))

        # transparently convert fields that are just a type have no options to
        # the rich field type for internal use
        if not hasattr(f, "type") or _is_union(f):
            fields[name] = _field(f)

    if "__I_AM_TARGET__" in fields:
        fields.pop("__I_AM_TARGET__", None)
        return struct(
            __I_AM_TARGET__ = True,
            fields = fields,
        )

    shape_struct = struct(
        fields = fields,
    )

    # the name of this function is important and makes the
    # backwards-compatibility hack in _new_shape work!
    def shape_constructor_function(
            __internal_get_shape = False,
            **kwargs):
        # starlark does not allow attaching arbitrary data to a function object,
        # so we have to make these internal parameters to return it
        if __internal_get_shape:
            return shape_struct
        return _new_shape(shape_struct, **kwargs)

    return shape_constructor_function

def _is_shape_constructor(x):
    """Check if input x is a shape constructor function"""

    # starlark doesn't have callable() so we have to do this
    if ((repr(x).endswith("antlir/bzl/shape.bzl.shape_constructor_function")) or  # buck2
        (repr(x) == "<function shape_constructor_function>") or  # buck1
        repr(x).startswith("<function _shape.<locals>.shape_constructor_function")):  # python mock
        return True
    return False

def _normalize_type(x):
    if _is_shape_constructor(x):
        return x(__internal_get_shape = True)
    return x

def _is_shape(x):
    if not structs.is_struct(x):
        return False
    if not hasattr(x, "fields"):
        return False
    if hasattr(x, "__I_AM_TARGET__"):
        return True
    return list(structs.to_dict(x).keys()) == ["fields"]

def _shape_defaults_dict(shape):
    defaults = {}
    for key, field in shape.fields.items():
        if field.default != _NO_DEFAULT:
            defaults[key] = field.default
    return defaults

def _new_shape(shape, **fields):
    """
    Type check and instantiate a struct of the given shape type using the
    values from the **fields kwargs.

    Example usage:
    ```
    example_t = shape.shape(hello=str)
    example = shape.new(example_t, hello="world")
    ```
    """

    # if this looks like the new constructor api, call it as a function
    if _is_shape_constructor(shape):
        return shape(**fields)

    with_defaults = _shape_defaults_dict(shape)

    # shape.bzl uses often pass shape fields around as kwargs, which makes
    # us likely to pass in `None` for a shape field with a default, provide
    # `shape.DEFAULT_VALUE` as a sentinel to make functions wrapping shape
    # construction easier to manage
    fields = {k: v for k, v in fields.items() if v != _DEFAULT_VALUE}
    with_defaults.update(fields)

    for field, value in fields.items():
        if field not in shape.fields:
            fail("field `{}` is not defined in the shape".format(field))
        error = _check_type(value, shape.fields[field])
        if error:
            fail("field {}, value {}: {}".format(field, value, error))

    return struct(
        __shape__ = shape,
        **with_defaults
    )

def _impl(name, deps = (), visibility = None, expert_only_custom_impl = False, **kwargs):  # pragma: no cover
    if not name.endswith(".shape"):
        fail("shape.impl target must be named with a .shape suffix")
    export_file(
        name = name + ".bzl",
        antlir_rule = "user-internal",
    )

    buck_genrule(
        name = name,
        antlir_rule = "user-internal",
        cmd = """
            $(exe {}) {} $(location :{}.bzl) {} > $OUT
        """.format(
            antlir_dep("bzl/shape2:bzl2ir"),
            normalize_target(":" + name),
            name,
            shell.quote(repr({d: "$(location {})".format(d) for d in deps})),
        ),
    )

    ir2code_prefix = "$(exe {}) --templates $(location {})/templates".format(antlir_dep("bzl/shape2:ir2code"), antlir_dep("bzl/shape2:templates"))

    if not expert_only_custom_impl:
        buck_genrule(
            name = "{}.py".format(name),
            cmd = "{} pydantic $(location :{}) > $OUT".format(ir2code_prefix, name),
            antlir_rule = "user-internal",
        )
        python_library(
            name = "{}-python".format(name),
            srcs = {":{}.py".format(name): "__init__.py"},
            base_module = native.package_name() + "." + name.replace(".shape", ""),
            deps = [antlir_dep(":shape")] + ["{}-python".format(d) for d in deps],
            visibility = visibility,
            antlir_rule = "user-facing",
            **{k.replace("python_", ""): v for k, v in kwargs.items() if k.startswith("python_")}
        )
        buck_genrule(
            name = "{}.rs".format(name),
            cmd = "{} rust $(location :{}) > $OUT".format(ir2code_prefix, name),
            antlir_rule = "user-internal",
        )
        rust_library(
            name = "{}-rust".format(name),
            crate = kwargs.pop("rust_crate", name[:-len(".shape")]),
            mapped_srcs = {":{}.rs".format(name): "src/lib.rs"},
            deps = ["{}-rust".format(d) for d in deps] + [antlir_dep("bzl/shape2:shape")] + third_party.libraries(
                [
                    "anyhow",
                    "fbthrift",
                    "serde",
                    "serde_json",
                ],
                platform = "rust",
            ),
            visibility = visibility,
            antlir_rule = "user-facing",
            unittests = False,
            allow_unused_crate_dependencies = True,
            **{k.replace("rust_", ""): v for k, v in kwargs.items() if k.startswith("rust_")}
        )

_SERIALIZING_LOCATION_MSG = (
    "shapes with layer/target fields cannot safely be serialized in the" +
    " output of a buck target.\n" +
    "For buck_genrule uses, consider passing an argument with the (shell quoted)" +
    " result of 'shape.do_not_cache_me_json'\n" +
    "For unit tests, consider setting an environment variable with the same" +
    " JSON string"
)

# Does a recursive (deep) copy of `val` which is expected to be of type
# `t` (in the `shape` sense of type compatibility).
#
# `opts` changes the output as follows:
#
#   - Set `opts.include_dunder_shape == False` to strip `__shape__` from the
#     resulting instance structs.  This is desirable when serializing,
#     because that field will e.g. fail with `structs.as_json()`.
#
#   - `opts.on_target_fields` has 3 possible values:
#
#     * "preserve": Leave the field as a `//target:path` string.
#
#     * "fail": Fails at Buck parse-time. Used for scenarios that cannot
#       reasonably support target -> buck output path resolution, like
#       `shape.json_file()`.  But, in the future, we should be able to
#       migrate these to a `target_tagger.bzl`-style approach.
#
#     * "uncacheable_location_macro"`, this will replace fields of
#       type `Target` with a struct that has the target name and its on-disk
#       path generated via a `$(location )` macro.  This MUST NOT be
#       included in cacheable Buck outputs.
#
#     * "tag_targets", this will replace fields of type `Target` with a struct
#       produced by `target_tagger.tag_target` function. That structure can be
#       converted to a `feature` later on for safe passing to the antlir `compiler`.
#

def _recursive_copy_transform(val, t, opts):
    if hasattr(t, "__I_AM_TARGET__"):
        if opts.on_target_fields == "fail":
            fail(_SERIALIZING_LOCATION_MSG)
        elif opts.on_target_fields == "uncacheable_location_macro":
            return struct(
                name = val,
                path = "$(location {})".format(val),
            )
        elif opts.on_target_fields == "tag_targets":
            if (opts.target_tagger == None):  # pragma: no cover
                fail("`target_tagger` is a rquiered parameter for `tag_targets`")

            return {
                "path": target_tagger_helper.tag_target(opts.target_tagger, val),
            }
        elif opts.on_target_fields == "collect_deps":
            opts.deps[val] = 1
            return {
                "path": {"__BUCK_TARGET": normalize_target(val)},
            }
        elif opts.on_target_fields == "preserve":
            return val
        fail(
            # pragma: no cover
            "Unknown on_target_fields: {}".format(opts.on_target_fields),
        )
    elif _is_shape(t):
        error = _check_type(val, t)
        if error:  # pragma: no cover -- an internal invariant, not a user error
            fail(error)
        new = {}
        for name, field in t.fields.items():
            new[name] = _recursive_copy_transform(
                # The `_is_instance` above will ensure that `getattr` succeeds
                getattr(val, name),
                field,
                opts,
            )
        if opts.include_dunder_shape:
            if val.__shape__ != t:  # pragma: no cover
                fail("__shape__ {} didn't match type {}".format(
                    _pretty(val.__shape__),
                    _pretty(t),
                ))
            new["__shape__"] = t
        return struct(**new)
    elif _is_field(t):
        if t.optional and val == None:
            return None
        return _recursive_copy_transform(val, t.type, opts)
    elif _is_collection(t):
        if t.collection == dict:
            return {
                k: _recursive_copy_transform(v, t.item_type[1], opts)
                for k, v in val.items()
            }
        elif t.collection == list:
            return [
                _recursive_copy_transform(v, t.item_type, opts)
                for v in val
            ]

        # fall through to fail
    elif _is_union(t):
        matched_type, error = _find_union_type(val, t)
        if error:  # pragma: no cover
            fail(error)
        return _recursive_copy_transform(val, matched_type, opts)
    elif t == int or t == bool or t == str or t == _path or _is_enum(t):
        return val
    fail(
        # pragma: no cover
        "Unknown type {} for {}".format(_pretty(t), _pretty(val)),
    )

def _safe_to_serialize_instance(instance):
    return _recursive_copy_transform(
        instance,
        instance.__shape__,
        struct(include_dunder_shape = False, on_target_fields = "fail", target_tagger = None),
    )

def _do_not_cache_me_json(instance):
    """
    Serialize the given shape instance to a JSON string, which is the only
    way to safely refer to other Buck targets' locations in the case where
    the binary being invoked with a certain shape instance is cached.

    Warning: Do not ever put this into a target that can be cached, it should
    only be used in cmdline args or environment variables.
    """
    return structs.as_json(_recursive_copy_transform(
        instance,
        instance.__shape__,
        struct(
            include_dunder_shape = False,
            on_target_fields = "uncacheable_location_macro",
            target_tagger = None,
        ),
    ))

def _json_file(name, instance, visibility = None):  # pragma: no cover
    """
    Serialize the given shape instance to a JSON file that can be used in the
    `resources` section of a `python_binary` or a `$(location)` macro in a
    `buck_genrule`.

    Warning: this will fail to serialize any shape type that contains a
    reference to a target location, as that cannot be safely cached by buck.
    """
    instance = structs.as_json(_safe_to_serialize_instance(instance))
    buck_genrule(
        name = name,
        # Antlir users should not directly use `shape`, but we do use it
        # as an implementation detail of "builder" / "publisher" targets.
        antlir_rule = "user-internal",
        cmd = "echo {} > $OUT".format(shell.quote(instance)),
        visibility = visibility,
    )
    return normalize_target(":" + name)

def _render_template(name, instance, template, visibility = None):  # pragma: no cover
    """
    Render the given Jinja2 template with the shape instance data to a file.

    Warning: this will fail to serialize any shape type that contains a
    reference to a target location, as that cannot be safely cached by buck.
    """
    _json_file(name + "--data.json", instance)

    buck_genrule(
        name = name,
        antlir_rule = "user-internal",
        cmd = "$(exe {}-render) <$(location :{}--data.json) > $OUT".format(template, name),
        visibility = visibility,
    )
    return normalize_target(":" + name)

def _python_data(
        name,
        instance,
        shape_impl,
        type_name,
        module = None,
        **python_library_kwargs):  # pragma: no cover
    """
    Codegen a static shape data structure that can be directly 'import'ed by
    Python. The object is available under the name "data". A common use case
    is to call shape.python_data inline in a target's `deps`, with `module`
    (defaults to `name`) then representing the name of the module that can be
    imported in the underlying file.

    Example usage:
    ```
    python_binary(
        name = provided_name,
        deps = [
            shape.python_data(
                name = "bin_bzl_args",
                instance = shape.new(
                    some_shape_t,
                    var = input_var,
                ),
            ),
        ],
        ...
    )
    ```

    can then be imported as:

        from .bin_bzl_args import data
    """
    shape = instance.__shape__
    instance = _safe_to_serialize_instance(instance)
    module = module or name

    shape_target = target_utils.parse_target(normalize_target(shape_impl))
    shape_module = shape_target.base_path.replace("/", ".") + "." + shape_target.name.replace(".shape", "")

    buck_genrule(
        name = "{}.py".format(name),
        # Antlir users should not directly use `shape`, but we do use it
        # as an implementation detail of "builder" / "publisher" targets.
        antlir_rule = "user-internal",
        cmd = """
            echo "from {module} import {type_name}" > $OUT
            echo {data} >> $OUT
        """.format(
            data = shell.quote("data = {classname}.parse_raw({shape_json})".format(
                classname = type_name,
                shape_json = repr(structs.as_json(instance)),
            )),
            module = shape_module,
            type_name = type_name,
        ),
    )

    python_library(
        name = name,
        srcs = {":{}.py".format(name): "{}.py".format(module)},
        deps = [shape_impl + "-python"],
        # Antlir users should not directly use `shape`, but we do use it
        # as an implementation detail of "builder" / "publisher" targets.
        antlir_rule = "user-internal",
        **python_library_kwargs
    )
    return normalize_target(":" + name)

# Asserts that there are no "Buck target" in the shape.  Contrast with
# `do_not_cache_me_json`.
#
# Converts a shape to a dict, as you would expected (field names are keys,
# values are scalars & collections as in the shape -- and nested shapes are
# also dicts).
def _as_serializable_dict(instance):
    return _as_dict_deep(_safe_to_serialize_instance(instance))

# Do not use this outside of `target_tagger.bzl`.  Eventually, target tagger
# should be replaced by shape, so this is meant as a temporary shim.
#
# Unlike `as_serializable_dict`, does not fail on "Buck target" fields. Instead,
# these get represented as the target path (avoiding cacheability issues).
#
# target_tagger.bzl is the original form of matching target paths with their
# corresponding `$(location)`.  Ideally, we should fold this functionality
# into shape.  In the current implementation, it just needs to get the raw
# target path out of the shape, and nothing else.
# This function is DEPRECATED in favor of _as_target_tagged_dict.
# ToDo: get rid of it
def _as_dict_for_target_tagger(instance):
    return structs.to_dict(_recursive_copy_transform(
        instance,
        instance.__shape__,
        struct(
            include_dunder_shape = False,
            on_target_fields = "preserve",
            target_tagger = None,
        ),
    ))

# Returns instance in which all target_t shapes get converted to the tagged targets.
# Result might need to be converted to a feature by the caller later on.
def _as_target_tagged_dict(target_tagger, instance):
    return structs.to_dict(_recursive_copy_transform(
        instance,
        instance.__shape__,
        struct(
            include_dunder_shape = False,
            on_target_fields = "tag_targets",
            target_tagger = target_tagger,
        ),
    ))

# Collects targets from shape and converts them to tagged targets. Returns
# list of dependencies in shape and shape converted to dict. Used in buck2
# implementation of `genrule_layer`.
def _as_dict_collect_deps(instance):
    deps = {}
    shape_as_dict = _as_dict_deep(_recursive_copy_transform(
        instance,
        instance.__shape__,
        struct(
            include_dunder_shape = False,
            on_target_fields = "collect_deps",
            target_tagger = None,
            deps = deps,
        ),
    ))

    return shape_as_dict, list(deps.keys())

# Converts `shape.new(foo_t, x='a', y=shape.new(bar_t, z=3))` to
# `{'x': 'a', 'y': shape.new(bar_t, z=3)}`.
#
# The primary use-case is unpacking a shape in order to construct a modified
# variant.  E.g.
#
#   def new_foo(a, b=3):
#       if (a + b) % 1:
#           fail("a + b must be even, got {} + {}".format(a, b))
#       return shape.new(_foo_t, a=a, b=b, c=a+b)
#
#   def modify_foo(foo, ... some overrides ...):
#       d = shape.as_dict_shallow(instance)
#       d.update(... some overrides ...)
#       d.pop('c')
#       return new_foo(**d)
#
# Notes:
#   - This dict is NOT intended for serialization, since nested shape remain
#     as shapes, and are not converted to `dict`.
#   - There is no special treament for `shape.target` fields, they remain as
#     `//target:path` strings.
#   - `shape.new` is the mathematical inverse of `_as_dict_shallow`.  On the
#     other hand, we do not yet provide `_as_dict_deep`.  The latter would
#     NOT be invertible, since `shape` does not yet have a way of
#     recursively converting nested dicts into nested shapes.
def _as_dict_shallow(instance):
    return {
        field: getattr(instance, field)
        for field in instance.__shape__.fields
    }

# Recursively converts nested shapes and structs to dicts. Used in shape.hash.
def _as_dict_deep(val, on_target_fields = "preserve"):
    if _is_any_instance(val):
        val = _recursive_copy_transform(
            val,
            val.__shape__,
            struct(
                include_dunder_shape = False,
                on_target_fields = on_target_fields,
                target_tagger = None,
            ),
        )
    if structs.is_struct(val):
        val = structs.to_dict(val)
    if types.is_dict(val):
        val = {k: _as_dict_deep(v) for k, v in val.items()}
    if types.is_list(val):
        val = [_as_dict_deep(item) for item in val]

    return val

# This function guarantees that the output json is a deterministic function of the input. It
# will also fail if the user attempts to pass a shape in that contains a Buck target. This is
# used in cases where we include the hash of a shape in a target name, which must always
# be deterministic.
def _stable_json(instance):
    if _is_any_instance(instance):
        instance = _as_dict_deep(instance, on_target_fields = "fail")

    if types.is_dict(instance):
        tokens = ["{}:{}".format(_stable_json(k), _stable_json(v)) for k, v in instance.items()]
        return "{{{}}}".format(",".join(tokens))
    elif types.is_list(instance):
        tokens = [_stable_json(v) for v in instance]
        return "[{}]".format(",".join(tokens))
    elif instance == None:
        return "null"
    elif types.is_string(instance):
        return '"{}"'.format(instance)
    elif types.is_bool(instance):
        return str(instance).lower()
    else:
        return str(instance)

# Returns True iff `instance` is a shape instance of any type.
def _is_any_instance(instance):
    return structs.is_struct(instance) and hasattr(instance, "__shape__")

def _hash_helper(val):  # pragma: no cover
    if types.is_dict(val):
        return sorted([(k, _hash_helper(v)) for k, v in val.items()])
    if types.is_list(val):
        return sorted([_hash_helper(v) for v in val])
    return val

# Generates a deterministic hash of a shape by recursively sorting every nested
# dict/list and hashing the resulting string.
def _hash(instance):  # pragma: no cover
    return sha256_b64(str(_hash_helper(_as_dict_deep(instance))))

shape = struct(
    # generate implementation of various client libraries
    impl = _impl,
    DEFAULT_VALUE = _DEFAULT_VALUE,
    # output target macros and other conversion helpers
    DEPRECATED_as_dict_for_target_tagger = _as_dict_for_target_tagger,
    as_dict_collect_deps = _as_dict_collect_deps,
    as_dict_shallow = _as_dict_shallow,
    as_serializable_dict = _as_serializable_dict,
    as_target_tagged_dict = _as_target_tagged_dict,
    dict = _dict,
    do_not_cache_me_json = _do_not_cache_me_json,
    enum = _enum,
    field = _field,
    hash = _hash,
    is_any_instance = _is_any_instance,
    is_instance = _is_instance,
    is_shape = _is_shape,
    json_file = _json_file,
    list = _list,
    new = _new_shape,
    path = _path,
    pretty = _pretty,
    python_data = _python_data,
    render_template = _render_template,
    shape = _shape,
    stable_json = _stable_json,
    union = _union,
    union_t = _union_type,
)
