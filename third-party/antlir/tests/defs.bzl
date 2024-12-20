load("//antlir/antlir2/bzl:hoist.bzl", "hoist")
load("//antlir/antlir2/bzl/feature:defs.bzl", "feature")
load("//antlir/bzl:build_defs.bzl", "buck_genrule", "buck_sh_test")
load("//antlir/bzl:third_party.bzl", "third_party")

def hello_world_build_test(ver, msg, patches = None):
    third_party.build(
        name = "hello_world.{}.build".format(ver),
        src = ":hello_world.tar",
        features = [feature.rpms_install(rpms = ["gcc", "patch"])],
        script = third_party.script(
            build = "gcc -o hello_world hello_world.c",
            install = "./hello_world > $OUTPUT/hello_world.out",
            patches = patches,
        ),
    )

    hoist(
        name = "hello_world.{}.out".format(ver),
        layer = ":hello_world.{}.build".format(ver),
        path = "/hello_world.out",
    )

    buck_genrule(
        name = "hello_world.{}.verify.sh".format(ver),
        out = "hello_world.{}.verify.sh".format(ver),
        bash = """
out_file=$(location {out_file})
out_msg=\\$(cat $out_file)

cat << EOF > "$OUT"
#!/bin/sh
set -x
[[ "$out_msg" == '{msg}' ]] && exit 0 || exit 1
EOF

chmod +x "$OUT"
""".format(
            msg = msg,
            out_file = ":hello_world.{}.out".format(ver),
        ),
        cacheable = False,
    )

    buck_sh_test(
        name = "hello_world.{}.test".format(ver),
        test = ":hello_world.{}.verify.sh".format(ver),
    )
