# Pure discovery path tests

```sh
rustc --edition=2021 --test tests/zcode-discovery-paths/harness.rs -o /tmp/loongport-zcode-path-tests
/tmp/loongport-zcode-path-tests
```

No filesystem, environment, credentials or native processes are accessed. These
tests verify path construction, not trusted discovery or account admission.
The prepared path functions remain outside the production module graph until
the native adapter calls them. Only the shared desktop trim utility is wired
into the existing production codec; no dead-code lint suppression is added.
