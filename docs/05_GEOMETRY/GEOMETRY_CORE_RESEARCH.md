# Geometry Core Research

*Исследование open-source геометрических ядер: CGAL, libigl, Cutter,
conformal-stencil-generator, OpenCADKernel.*

## Статус: направление не выбрано

**Это не план.** Документ хранит исследование, чтобы оно не потерялось, и не
утверждает, что что-то из перечисленного будет сделано.

Причины, по которым он не встроен в трекер:

* **Противоречит принятому решению A1** — ядро и Studio, сервисный слой
  отложен. Здесь предлагаются Go C ABI (`GEOM-070`) и коммерческий CGAL,
  то есть другое направление, а не детализация A1.
* **Не проходил сверку с кодом.** Задачи написаны без проверки того, что уже
  есть: часть предлагаемого (FFI для libigl, интеграция Cutter) уже либо
  не нужна, либо сделана иначе.
* **Часть критериев не проверяема прогоном** — «демо-видео», «99.9% успешных
  прогонов», «< 5 с на 200k треугольников».
* **Форма и язык** отличаются от остальной документации проекта.

## Что уже извлечено и закрыто

`GEOM-010` (аудит лицензий) закрыт и вынесен в
[`docs/LICENSES/LICENSE_COMPLIANCE.md`](../LICENSES/LICENSE_COMPLIANCE.md). Он от
направления не зависел: аудит показал, что все зависимости permissive, а
CGAL и libigl отвергнуты с основанием. Именно поэтому этот документ не нужно
выбрасывать целиком.

## Если направление когда-нибудь будет выбрано

Сначала задачи придётся перепроверить против кода — они писались без него, и
часть устарела бы. Затем встроить их в волны через граф зависимостей, как
остальные.

---

## Сверка с кодом (2026-10-09)

Задачи написаны без проверки дерева, поэтому каждая сверена с кодом отдельно.
Ключевой факт: **цепочка спроектирована под продукт, которого нет** — 3D-экспорт
помечен NOT NEEDED, а UV-параметризации в проекте не существует вовсе.

| Задача | Что обещает | Что в коде | Вердикт |
|---|---|---|---|
| `GEOM-020` | `shape_centroid`, `shape_moment_of_inertia`, `contour_centroid`, `shape_outer_bbox` | `centroid`/`moment`/`inertia` — **ни одного вхождения в engine**; `bounding_box` и `shape_bounds` уже есть; тестов **201**, требуется «20+»; rustdoc есть (43 в `region_cache`, 28 в `integrity`) | ⚠️ **частично устарела** — `shape_outer_bbox` дублирует существующее, остальное отсутствует и полезно |
| `GEOM-030` | libigl LSCM FFI, 3D→2D UV | нет `lscm`, `uv_*`, `parametri` — **нуля вхождений**. `cargo metadata` не показывает libigl: зависимости не подключены | ❌ **неактуально** — обслуживает `WEB-EXPORT`, помеченный NOT NEEDED |
| `GEOM-040` | bridging / kerf / despeckle из Cutter | `bridge_unanchored` ✓, `offset_shape` ✓ (kerf), `unanchored_islands` ✓, `drop_small_regions` ✓ — всё в ядре | ✅ **сделано** — предлагает существующее |
| `GEOM-050` | декомпозиция слоёв, UI мостов | `unanchored_islands` ✓, `bridge_unanchored` ✓; слоёв и UI нет | ⚠️ **частично** — алгоритмическая база есть, редактора нет |
| `GEOM-060` | профили Material/Cutter/Paint | не реализовано; `VALIDATE-001` уже делает геометрию→вердикт | ⚠️ **реальная работа** — согласуется с `VALIDATE-001` |
| `GEOM-070` | Go C ABI / gRPC | противоречит решению **А1** (сервисный слой отложен) | ❌ **противоречит принятому решению** |
| `GEOM-080` | MVP mesh → UV → pattern → SVG | держится на UV (нет) и SVG (NOT NEEDED) | ❌ **недостижимо** — оба основания отсутствуют |
| `GEOM-090` | продакшн-харднинг | golden-тесты есть (`scripts/golden/hashes.json`, регенератор в wasm), `make check` есть; **нет** property-based тестов, проверок утечек, многопоточности | ⚠️ **частично** — LSCM-профилирование отпадает вместе с `GEOM-030` |
| `GEOM-100` | интеграция Studio | зависит от всей перечисленной цепочки | ❌ **недостижимо** |

**Итог:** из девяти задач три уже сделаны или дублируют существующее
(`020` частично, `040`, `090` частично), пять обслуживают отменённую функцию или
противоречат решению (`030`, `050` частично, `070`, `080`, `100`), и **одна —
`GEOM-060` — нетривиальная работа, которой действительно нет**.

Про `shape_centroid` и `shape_moment_of_inertia`: отсутствуют, и обе полезны
независимо от исчезнувшего направления — первая для раскладки, вторая для
устойчивости разложения. Их стоит забрать в основной трекер отдельно от
остального блока.

---

*Ниже — текст исследования дословно, включая заметку о гибридной архитектуре.*

---

## Неподтверждённое направление: GEOMETRY CORE RESEARCH

*P0/A1 decision: hybrid architecture (existing Rust kernel + libigl + custom Mask Domain Core). CGAL RED FLAG (commercial license required). libigl MPL-2.0 usable. Cutter algorithms reusable (same i_overlay backend). conformal-stencil-generator too experimental. OpenCADKernel early stage MPL-2.0.*

> **Не является частью плана.** Блок сохранён дословно, чтобы решение не потерялось
> вместе с правкой. Он не прошёл сверку с кодом, не встроен в волны и местами
> противоречит принятому решению **A1** — см. **DOC-008**. Ни одна задача отсюда
> не может считаться начатой.

These tasks follow the architecture decision research and must be worked in stage order. Each stage depends on the previous one being complete.

### GEOM-010 · License audit и compliance documentation · DONE 2026-10-09

**Dependencies**: None

**Description**: 
Confirm all third-party license terms and document compliance requirements for commercial closed-source product.
- Verify libigl MPL-2.0 allows commercial closed source (with source object file obligation)
- Document CGAL cannot be used without commercial license (GeometryFactory contact)
- Verify i_overlay license compatibility
- Create License Compliance document with all dependencies and terms

**Acceptance criteria**:
- License audit table completed for all crates
- CGAL explicitly marked as NOT USED with reasoning
- libigl compliance mechanism documented (source offer protocol)
- Documentation reviewed and signed off

**Definition of Done**:
- `License_Compliance.md` created in docs/
- All `Cargo.lock` dependencies audited
- Legal sign-off obtained or path to obtainment documented

**Закрыто.** Документ: `docs/LICENSES/LICENSE_COMPLIANCE.md`. Таблица аудита — все 53 сторонних крейта из `Cargo.lock`, сгенерирована из `cargo metadata`, а не переписана руками.

**Ответ на первый вопрос: да, всё, что используется, можно распространять.** Каждая зависимость permissive — MIT, Apache-2.0, 0BSD, Zlib, Unlicense или их дизъюнкция. Ничего copyleft, поэтому ничто не заставляет публиковать наш исходный код.

Единственная зависимость вне обычной экосистемы Rust — `i_overlay` (MIT OR Apache-2.0), которую ядро реально использует для булевых операций и смещений (`kernel/src/boolean.rs`, `geom.rs`, `offset.rs`). Коммерческое закрытое использование разрешено напрямую.

Две лицензии с условием, которые стоит назвать вслух, а не спрятать за словом «permissive»: `unicode-ident` — `(MIT OR Apache-2.0) AND Unicode-3.0` (два независимых разрешения, а не одно); `version_check` — `MIT/Apache-2.0` со слешем, который часть инструментов читает как конъюнкцию.

**Ответ на второй вопрос — что отвергнуто и почему:**

| Кандидат | Решение | Основание |
|---|---|---|
| **CGAL** | **НЕ ИСПОЛЬЗУЕТСЯ** | Требует коммерческую лицензию от GeometryFactory для закрытого исходного кода. В дереве зависимостей отсутствует — проверено `cargo metadata`. |
| **libigl** | **НЕ ИСПОЛЬЗУЕТСЯ** | MPL-2.0: коммерческое закрытое использование разрешено, но это файловый copyleft — при линковке нужно предложить исходники тех файлов libigl, которые мы распространяем. FFI отсутствует. |
| **Cutter** | алгоритмы прочитаны, адаптированы вручную | Код из библиотеки не компилируется, поэтому её лицензия на продукт не распространяется. |
| **conformal-stencil-generator** | **НЕ ИСПОЛЬЗУЕТСЯ** | Слишком экспериментальный. |
| **OpenCADKernel** | **НЕ ИСПОЛЬЗУЕТСЯ** | Ранняя стадия, в дереве отсутствует. |

**Что осталось незакрытым и почему это не дыра:** атрибуция. Тексты лицензий не собраны нигде, `NOTICE` отсутствует, `docs/LICENSES/` пуст. Это работа, а не открытый вопрос — но она не сделана, и в документе это записано как следующий шаг, а не как решённое.

**Юридическое заключение: не получено.** Аудит говорит, что все зависимости permissive — это вывод. Подтверждение юристом для коммерческого релиза — решение владельца, и этот документ его не заменяет. Блокер не технический.

**Почему эта задача закрыта, хотя блок GEOM-* не подтверждён:** аудит лицензий от направления не зависит. Он покрывает `i_overlay`, который используется сейчас, и фиксирует отказ от CGAL и libigl, чтобы их не предлагали заново. Даже если блок GEOM-* будет удалён, документ остаётся.

---

### GEOM-020 · Fix kernel: add missing API functions · P1 · MEDIUM

**Dependencies**: None (existing kernel code)

**Description**:
Document and stabilize the existing Rust geometry kernel API. Add missing utility functions identified during research.
- Add `shape_centroid` — mass centroid calculation for nesting
- Add `shape_moment_of_inertia` — stability analysis for layer decomposition
- Add `contour_centroid` — UV seam placement helper
- Add `shape_outer_bbox` — bounding box extraction
- Generate Rustdocs for all pub modules
- Add 20+ boundary/edge case tests

**Acceptance criteria**:
- All new functions documented with examples
- 90%+ test coverage on kernel crate
- `cargo doc --no-deps --launch` succeeds without warnings
- New functions tested with boundary conditions (NaN, Inf, max coords, empty shapes)

**Definition of Done**:
- Kernel API documented and stable
- Test suite passing with new coverage
- No API-breaking changes planned without ADR

---

### GEOM-030 · Integrate libigl LSCM FFI · P1 · HIGH

**Dependencies**: GEOM-010 (license compliance confirmed)

**Description**:
Build Rust↔C++ FFI bridge for libigl surface parameterization. Enable 3D→2D UV mapping pipeline.
- Create C API wrappers for `igl::lscm()` and `igl::harmonic_parameterization()`
- Design memory-safe FFI boundary: caller-owned buffers, explicit size params
- Implement `IgFeature` enum: `LSCM`, `HARMONIC`, `ARAP`
- Implement `IgParams` struct: boundary indices, boundary coords, max iterations
- Implement `IgResult` struct: UV coords, convergence info, error code
- Test with helmet mesh (~100k tris), motorcycle tank (~200k tris), car hood (~500k tris)

**Acceptance criteria**:
- FFI bridge compiles on linux/macOS/Windows
- LSCM produces UV coords for test meshes
- Distortion metrics reported (max angle distortion, area distortion)
- Seam selection UI works (2+ points per connected component)
- Performance: < 5s for 200k triangle mesh

**Definition of Done**:
- FFI bridge in `engine/ffi/` directory
- `cargo test --lib` passes for FFI tests
- Demo script: `mesh_to_uv.sh` working
- Performance profile recorded

---

### GEOM-040 · Adapt Cutter bridging/kerf algorithms · P2 · MEDIUM

**Dependencies**: GEOM-020 (kernel stable)

**Description**:
Extract and adapt stencil generation algorithms from cutter library. Bridging, kerf compensation, min-feature despeckle — all use same i_overlay backend as kernel.
- Extract bridging logic: `bridge_unanchored` adaptation to kernel Shapes API
- Extract kerf compensation: offset_shape with kerf_mm parameter
- Extract min-feature despeckle: island merge algorithm from cutter stencil.rs
- Maintain physical constraints: min_material, kerf, bridge_length
- Port test suite from cutter to kernel test format

**Acceptance criteria**:
- Bridging produces same results as cutter on test patterns (within 0.01mm)
- Kerf compensation correctly shrinks geometry by blade width
- Despeckle removes islands below min_feature_size threshold
- All adapted functions pass kernel test suite

**Definition of Done**:
- Adapted algorithms in `engine/kernel/src/adapted/` directory
- Test suite passing (`cargo test --lib`)
- Comparison test: kernel vs cutter output on 5 test patterns

---

### GEOM-050 · Layer decomposition with bridges/islands · P2 · HIGH

**Dependencies**: GEOM-030 (libigl FFI), GEOM-040 (adapted algorithms)

**Description**:
Implement mask domain logic: decompose 2D pattern into physical paint layers with bridges, islands, peel direction.
- Island detection: integrate `measure::unanchored_islands` into UI
- Bridge placement: integrate `bridge::bridge_unanchored` with manufacturing profile
- Peel direction analysis: algorithm for paint order determination
- Layer decomposition: main algorithm — pattern → ordered layers with bridges
- UI components: layer editor, bridge editor, registration marks

**Acceptance criteria**:
- Input: 2D pattern in UV space
- Output: Ordered layers with bridges, islands handled, registration marks
- Each layer passes min_width, min_gap, bridge validation
- Paint order logical (based on area, complexity, peel direction)
- UI can edit bridges (add/remove/relocate)

**Definition of Done**:
- Layer decomposition algorithm implemented and tested
- UI components for layer/bridge editing
- Test scenarios: helmet, motorcycle tank, car hood patterns
- Export to mask project format with layer data

---

### GEOM-060 · Manufacturing profiles system · P2 · MEDIUM

**Dependencies**: GEOM-050 (layer decomposition working)

**Description**:
Build profile-driven geometry validation. Material constraints, cutter characteristics, paint properties drive geometry decisions.
- MaterialProfile: min_width_mm, min_gap_mm, min_island_mm2, min_bridge_mm, kerf_mm
- CutterProfile: offset_curve, fillet_radius, miter_limit, tolerance_mm
- PaintProfile: opacity, layer_order (auto/commanual), bleed_compensation, dry_time
- Profile management UI: load/save JSON, profile library (standard profiles)
- Profile validation: geometry → verdict + violations mapping

**Acceptance criteria**:
- Profiles load from JSON files
- Geometry validation returns verdict + specific violations
- Profile-driven tolerance for LSCM curve approximation
- UI can create/edit/delete profiles
- Standard profiles: vinyl-090, knife-25mm, matte-vinyl-090

**Definition of Done**:
- Profile data models implemented
- Profile management UI
- Validation pipeline: geometry + profile → verdict
- Test profiles with known pass/fail cases

---

### GEOM-070 · Go C ABI integration · P2 · MEDIUM

**Dependencies**: GEOM-030 (libigl FFI), platform toolchain

**Description**:
Enable geometry calls from Go backend. Two approaches: CGO or separate service.
- C ABI bindings: compile libigl + kernel as shared library
- CGO test: Go file calling Rust geometry functions
- Alternative: gRPC service protocol (if distributed needed)
- WASM fallback: compile to wasm32-unknown-unknown for browser
- CLI tool: `mask-engine geo` commands for Go integration testing

**Acceptance criteria**:
- Go can call kernel geometry functions via C ABI
- OR gRPC service running with geometry operations
- WASM module compiles and runs in browser
- CLI tool working for all geometry operations

**Definition of Done**:
- C ABI working (or gRPC service)
- Go test program calling geometry functions
- WASM build and browser test
- CLI `mask-engine geo --help` showing all options

---

### GEOM-080 · MVP Geometry Core · P1 · HIGH

**Dependencies**: GEOM-020 through GEOM-070 (all previous stages)

**Description**:
Assemble all components into working MVP. Full pipeline: 3D mesh → LSCM UV → pattern → layered SVG with bridges/islands. Validation: min_width, min_gap, bridges present.
- Combine kernel + libigl FFI + layer decomposition + bridging
- SVG export pipeline: 3D mesh → UV → pattern → layered SVG
- Validation pipeline: geometry + profile → verdict + messages
- Test scenarios: helmet, motorcycle tank, car hood
- CLI: `mask-engine full-pipeline --mesh car.obj --pattern design.svg --profile vinyl-v4.json --output manifest.json`

**Acceptance criteria**:
- Full pipeline working end-to-end
- All three test scenarios pass validation
- CLI commands working as described
- SVG output physically manufacturable (all constraints satisfied)

**Definition of Done**:
- MVP `cargo run --release` completes without errors
- All test scenarios: PASS verdict with specific measurements
- CLI help and commands documented
- MVP demo video or screenshot

---

### GEOM-090 · Production hardening · P2 · MEDIUM

**Dependencies**: GEOM-080 (MVP working)

**Description**:
Performance optimization, robustness improvements, test suite, security review.
- Profile LSCM computation for 1M+ triangles
- Memory optimization: streaming vs full mesh
- Multithreading for parallel computation
- Property-based tests (QuickCheck/rust-prophets)
- Golden tests: compare output with reference data
- CI integration: `make check`, cargo test, linting
- FFI boundary validation: no invalid values cross C++ boundary
- Memory leak checks

**Acceptance criteria**:
- Performance: < 30s для full car body (1M triangles)
- Reliability: 99.9% success rate на test suite
- License compliance: documented and verified
- Full test suite passing
- No security vulnerabilities in FFI boundary

**Definition of Done**:
- Performance profile recorded for all key operations
- Property-based test suite passing
- CI pipeline green on every push
- Security review completed

---

### GEOM-100 · Studio integration · P2 · MEDIUM

**Dependencies**: GEOM-080 (MVP working)

**Description**:
Frontend components, API endpoints, WASM module.
- Next.js: drag&drop 3D model viewer (Three.js/react-three-fiber)
- UV editor: move/add seams
- Layer editor: toggles, bridge editor
- Preview: 3D visualization of applied pattern
- API: POST /api/geo/parameterize, POST /api/geo/decompose, POST /api/geo/validate
- WASM module: browser-based computation без backend

**Acceptance criteria**:
- 3D model viewer with drag&drop
- UV editor with seam selection
- Layer editor with bridge control
- API endpoints working (tested with curl)
- WASM module in browser demo

**Definition of Done**:
- Studio frontend with geometry operations
- API endpoints tested and documented
- WASM demo in browser
- User flow: upload mesh → get layered SVG

---

