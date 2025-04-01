derivation {
    name = "unstable-drv";
    builder = "/bin/sh";
    system = builtins.currentSystem;
    args = ["-c" "set -x; sleep ${toString builtins.currentTime}; echo hiya > $out; echo lol > $bin"];
   outputs = ["out" "bin"];
}
