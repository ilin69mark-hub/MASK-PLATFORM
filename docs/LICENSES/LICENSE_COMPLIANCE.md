# License Compliance

*GEOM-010 in `docs/BACKLOG/MASTER_BACKLOG.md`. Date: 2026-10-09.*

## What this is for

This product is commercial and closed source. Every dependency it carries has to
be licensed for that, and the ones we *considered* have to be on record as
considered, so nobody re-proposes them six months from now and has to start from
the beginning.

Two questions, answered here:

1. Is what we already use safe to ship?
2. What did we look at and refuse, and on what grounds?

## Answer to the first question

**Yes. Every dependency in the tree is permissively licensed** — MIT, Apache-2.0,
0BSD, Zlib, Unlicense, or a disjunction of those. Nothing is copyleft, so nothing
forces us to publish our own source, and nothing obliges us to relicense.

The one dependency that is not part of the ordinary Rust ecosystem is `i_overlay`,
which the geometry kernel actually uses for boolean and offset operations
(`kernel/src/boolean.rs`, `geom.rs`, `offset.rs`). It is `MIT OR Apache-2.0`, which
permits closed-source commercial use outright.

Two licenses carry a condition worth saying out loud rather than hiding behind
the word "permissive":

| Крейт | Условие |
|---|---|
| `unicode-ident` | `(MIT OR Apache-2.0) AND Unicode-3.0` — a second, independent grant. Permissive, but it is two licenses, not one. |
| `version_check` | `MIT/Apache-2.0` — the SPDX id carries a slash, which some tooling reads as a conjunction rather than a disjunction. Harmless here, worth knowing. |

## Dependency audit

Every crate in `Cargo.lock`, from `cargo metadata`. Regenerate with:

```text
cd engine && cargo metadata --format-version 1
```

| Крейт | Версия | Лицензия |
|---|---|---|
| adler2 | 2.0.1 | `0BSD OR MIT OR Apache-2.0` |
| bitflags | 2.13.2 | `MIT OR Apache-2.0` |
| block-buffer | 0.10.4 | `MIT OR Apache-2.0` |
| bumpalo | 3.20.3 | `MIT OR Apache-2.0` |
| cfg-if | 1.0.5 | `MIT OR Apache-2.0` |
| cpufeatures | 0.2.17 | `MIT OR Apache-2.0` |
| crc32fast | 1.5.2 | `MIT OR Apache-2.0` |
| crypto-common | 0.1.7 | `MIT OR Apache-2.0` |
| digest | 0.10.7 | `MIT OR Apache-2.0` |
| fdeflate | 0.3.7 | `MIT OR Apache-2.0` |
| flate2 | 1.1.10 | `MIT OR Apache-2.0` |
| futures-core | 0.3.34 | `MIT OR Apache-2.0` |
| futures-task | 0.3.34 | `MIT OR Apache-2.0` |
| futures-util | 0.3.34 | `MIT OR Apache-2.0` |
| generic-array | 0.14.7 | `MIT` |
| i_float | 5.0.0 | `MIT` |
| i_key_sort | 0.11.0 | `MIT` |
| i_overlay | 9.0.0 | `MIT OR Apache-2.0` |
| i_shape | 5.0.0 | `MIT` |
| i_tree | 0.19.0 | `MIT` |
| itoa | 1.0.18 | `MIT OR Apache-2.0` |
| js-sys | 0.3.106 | `MIT OR Apache-2.0` |
| libc | 0.2.190 | `MIT OR Apache-2.0` |
| libm | 0.2.16 | `MIT` |
| memchr | 2.8.3 | `Unlicense OR MIT` |
| miniz_oxide | 0.8.9 | `MIT OR Zlib OR Apache-2.0` |
| miniz_oxide | 0.9.1 | `MIT OR Zlib OR Apache-2.0` |
| once_cell | 1.21.4 | `MIT OR Apache-2.0` |
| pin-project-lite | 0.2.17 | `Apache-2.0 OR MIT` |
| png | 0.18.1 | `MIT OR Apache-2.0` |
| proc-macro2 | 1.0.107 | `MIT OR Apache-2.0` |
| quote | 1.0.47 | `MIT OR Apache-2.0` |
| rustversion | 1.0.23 | `MIT OR Apache-2.0` |
| serde | 1.0.229 | `MIT OR Apache-2.0` |
| serde-wasm-bindgen | 0.6.5 | `MIT` |
| serde_core | 1.0.229 | `MIT OR Apache-2.0` |
| serde_derive | 1.0.229 | `MIT OR Apache-2.0` |
| serde_json | 1.0.151 | `MIT OR Apache-2.0` |
| sha2 | 0.10.9 | `MIT OR Apache-2.0` |
| simd-adler32 | 0.3.10 | `MIT` |
| slab | 0.4.12 | `MIT` |
| syn | 3.0.6 | `MIT OR Apache-2.0` |
| thiserror | 2.0.21 | `MIT OR Apache-2.0` |
| thiserror-impl | 2.0.21 | `MIT OR Apache-2.0` |
| typenum | 1.20.1 | `MIT OR Apache-2.0` |
| unicode-ident | 1.0.26 | `(MIT OR Apache-2.0) AND Unicode-3.0` |
| version_check | 0.9.5 | `MIT/Apache-2.0` |
| wasm-bindgen | 0.2.129 | `MIT OR Apache-2.0` |
| wasm-bindgen-macro | 0.2.129 | `MIT OR Apache-2.0` |
| wasm-bindgen-macro-support | 0.2.129 | `MIT OR Apache-2.0` |
| wasm-bindgen-shared | 0.2.129 | `MIT OR Apache-2.0` |
| zlib-rs | 0.6.8 | `Zlib` |
| zmij | 1.0.23 | `MIT` |

всего сторонних крейтов: 53

## Answer to the second question

| Кандидат | Решение | Основание |
|---|---|---|
| **CGAL** | **НЕ ИСПОЛЬЗУЕТСЯ** | Требует коммерческую лицензию от GeometryFactory для закрытого исходного кода. В дереве зависимостей отсутствует — проверено `cargo metadata`. Алгоритмы ядра покрывают то, что нам нужно; от CGAL ничего не требуется. |
| **libigl** | **НЕ ИСПОЛЬЗУЕТСЯ** | MPL-2.0: коммерческое закрытое использование разрешено, но это файловый copyleft — при линковке нужно предложить исходники тех файлов libigl, которые мы распространяем. FFI отсутствует, импорта нет. |
| **Cutter** | алгоритмы прочитаны, адаптированы вручную | Мосты, компенсация реза, удаление мелких элементов перенесены в ядро как *алгоритмы*. Код из библиотеки не компилируется, поэтому её лицензия на продукт не распространяется. |
| **conformal-stencil-generator** | **НЕ ИСПОЛЬЗУЕТСЯ** | Слишком экспериментальный. |
| **OpenCADKernel** | **НЕ ИСПОЛЬЗУЕТСЯ** | Ранняя стадия, MPL-2.0, в дереве отсутствует. |

The CGAL line is the one that matters most, because it is the library a geometry
project reaches for first and the one that would cost money. It is absent on
purpose and the reason is on record.

## Compliance mechanism

Nothing in the tree obliges us to publish source. What is still missing is
**attribution**: the license texts are not collected anywhere, and there is no
`NOTICE` naming what we ship.

`docs/LICENSES/` exists and is empty. Two things close the gap:

1. The license texts of the crates above go into `docs/LICENSES/<crate>.txt`.
2. A `NOTICE` lists the third-party components, so a recipient can see what the
   binary is made of without reading the lockfile.

## If libigl is ever adopted

MPL-2.0 allows closed-source use, so this is a process obligation rather than a
blocker. Before any `libigl` FFI lands:

- record exactly which libigl files are compiled in, and their versions;
- publish the offer of source for those files, in the distribution and for three
  years, as the license requires;
- keep the FFI boundary C-only. The copyleft is per-file, so C++ templates that
  pull our headers into a translation unit are where it stops being simple.

## Sign-off

**Legal review: outstanding.** The audit says every dependency is permissive, which
is the finding; a lawyer confirming that for a commercial release is the owner's
call and is not something this document can substitute for. The blocker is not
technical.
