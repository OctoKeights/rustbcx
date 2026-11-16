# Performance

[Overview](../README.md) · [Examples](examples.md) · [Guide](guide.md) · [Behaviour](behaviour.md) · [Verification](verification.md) · **Performance** · [Credits](credits.md)

---

- [Benchmarks](#benchmarks)
- [The `nightly-simd` feature](#the-nightly-simd-feature)
- [Comparison with the originals](#comparison-with-the-originals)

## Benchmarks

Throughput for a 512 × 512 image (16,384 blocks) on a two-core x86-64 machine, in thousands
of blocks per second, best of three runs. Reproduce the table on your own hardware with
`cargo run --release --example bench`.

The encoders' lookup tables are built once per process, in about 26 milliseconds, and are
excluded from these figures.

| Configuration | One thread | Two threads |
|---|---|---|
| BC1, level 5 | 3,160 | 5,000 |
| BC1, level 18 | 60 | 101 |
| BC3, level 10 | 136 | 290 |
| BC4 | 175 | 288 |
| BC5 | 113 | 192 |
| BC7, `bc7enc`, uber 0 | 169 | 310 |
| BC7, `bc7enc`, uber 4 | 72 | 132 |
| BC7, `bc7e`, `ultrafast` | 302 | 551 |
| BC7, `bc7e`, `basic` | 33 | 64 |
| BC7, `bc7e`, `slowest` | 8.2 | 15 |
| BC7, `bc7e` `basic`, with the rate-distortion pass | 9.8 | 20 |
| BC6H, `veryfast` | 1,200 | 1,890 |
| BC6H, `basic` | 80 | 153 |
| BC6H, `veryslow` | 6.5 | 12 |

## The `nightly-simd` feature

With a nightly compiler, the `nightly-simd` feature makes `bc7e` evaluate a set of endpoints
for four pixels at a time using `std::simd`, where the default build evaluates one:

```text
cargo +nightly build --release --features nightly-simd
```

The output is unchanged, and `bc7e` is about a tenth faster at its `basic` profile and above
(a twelfth fewer instructions at `slowest`).

The same loop written four pixels at a time without `std::simd`, leaving vectorisation to
the compiler, was evaluated for the default build and proved slower than the code it
replaced, which the compiler already vectorises across a palette's colours. The default
build therefore retains the original's loop.

`std::simd` is not yet stable, and a later nightly may require changes to the feature's few
lines. They were last built against the `std::simd` of Rust 1.97.

## Comparison with the originals

On one thread of the same machine:

- `bc7e` takes about four fifths of the time of the strict SSE2 build of `bc7e.ispc`, and
  about a fifth longer than its AVX2 fast-math build.
- BC6H takes about one and a half times as long as the strict SSE2 build of Intel's encoder,
  which processes four blocks at a time.

Treat these figures as indicative only: they reflect a single machine and synthetic pixels.
