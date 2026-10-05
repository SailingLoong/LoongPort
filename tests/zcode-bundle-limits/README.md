# Fixed .zsb parameter tests

Run without Cargo or account data:

```sh
rustc --edition=2021 --test tests/zcode-bundle-limits/harness.rs -o /tmp/loongport-zsb-limits-tests
/tmp/loongport-zsb-limits-tests
```

The harness imports product source and checks only format/parameter/resource
limits. It does not implement JSON parsing, password derivation, authenticated
decryption, native schema validation, account preview or vault import. Passing
these tests is not .zsb import acceptance or cross-machine compatibility.
