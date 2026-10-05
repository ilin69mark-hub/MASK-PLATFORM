# DOMAIN_MODEL

## 1. Границы домена

```mermaid
flowchart TB
  subgraph CORE["Mask Domain (Mask Engine)"]
    MP["MaskProject"]
    LY["Layer"]
    LG["LayerGraph"]
    GEO["Geometry"]
    VAL["Validation"]
    ART["Artifact"]
  end
  subgraph SUP["Supply Domain (Manufacturing)"]
    MAT["MaterialProfile"]
    PLO["PlotterProfile"]
    RUL["RuleSet"]
  end
  subgraph CAT["Catalog Domain"]
    OM["ObjectModel"]
    PT["Part"]
    SURF["Surface"]
  end
  subgraph CUST["Commerce Domain"]
    USR["User"]
    ORD["Order"]
  end
  MP --> GEO
  MP --> LY
  LY --> LG
  MP -.->|snapshot| MAT
  MP -.->|snapshot| PLO
  MAT --> RUL
  PLO --> RUL
  RUL --> VAL
  VAL --> MP
  MP --> OM
  OM --> PT
  PT --> SURF
  USR --> MP
  USR --> ORD
  MP --> ORD
```

**Правило зависимости:** ядро (`Mask Domain`) знает о `Supply` и `Catalog` только
**через снимки (snapshots)**. Если бы ядро читало каталог напрямую, его нельзя было бы
запустить офлайн и в браузере.

## 2. ERD

```mermaid
erDiagram
  USER ||--o{ MASK_PROJECT : owns
  USER ||--o{ ORDER : places

  OBJECT_MODEL ||--o{ PART : contains
  OBJECT_MODEL }o--|| BRAND : belongs
  PART ||--o{ SURFACE : has
  PART ||--o{ FORBIDDEN_ZONE : restricts
  SURFACE ||--o{ GEOMETRY_VERSION : versions

  MASK_PROJECT }o--|| OBJECT_MODEL : targets
  MASK_PROJECT }o--|| PART : targets
  MASK_PROJECT ||--o{ SOURCE_ASSET : has
  MASK_PROJECT ||--o{ ENGINE_RUN : produces
  MASK_PROJECT ||--o{ MASK_LAYER : contains
  MASK_PROJECT ||--|| LAYER_GRAPH : governs
  MASK_PROJECT ||--o{ PROFILE_SNAPSHOT : freezes
  MASK_PROJECT ||--o{ VALIDATION_REPORT : has
  MASK_PROJECT ||--o{ PROJECT_REVISION : versions
  MASK_PROJECT ||--o{ ARTIFACT : emits
  MASK_PROJECT ||--o{ NESTING_PLAN : cut_by
  MASK_PROJECT ||--o{ INSTRUCTION : describes

  MASK_LAYER ||--o{ GEOMETRY : defined_by
  MASK_LAYER ||--o{ PAINT_SPEC : painted_with
  MASK_LAYER ||--o{ LAYER_RELATION : participates
  LAYER_GRAPH ||--o{ LAYER_RELATION : contains
  GEOMETRY ||--o{ GEOMETRY_PATH : contains
  GEOMETRY_PATH ||--o{ GEOMETRY_NODE : vertices
  GEOMETRY_PATH ||--o{ GEOMETRY_CONTOUR : typed_as

  MATERIAL_PROFILE ||--o{ PROFILE_VERSION : versions
  PLOTTER_PROFILE ||--o{ PROFILE_VERSION : versions
  PROFILE_SNAPSHOT }o--|| PROFILE_VERSION : freezes
  RULE_SET }o--|| PROFILE_VERSION : derived_from

  VALIDATION_REPORT ||--o{ VIOLATION : lists
  VIOLATION }o--|| TOPOLOGY_RULE : describes

  NESTING_PLAN ||--o{ NEST_TILE : splits_into

  ENGINE_RUN ||--o{ ARTIFACT : produces
  ENGINE_RUN }o--|| ENGINE_VERSION : executed_by

  ORDER ||--o{ ORDER_ITEM : contains
  ORDER_ITEM }o--|| MASK_PROJECT : references
  ORDER ||--o{ NESTING_PLAN : fulfilled_by

  USER {
    uuid id PK
    string email UK
    string role
    tstz created_at
  }

  OBJECT_MODEL {
    uuid id PK
    string brand
    string model
    string generation
    int year_from
    int year_to
    string notes
  }

  PART {
    uuid id PK
    uuid object_model_id FK
    string code
    string name
    float real_width_mm
    float real_height_mm
  }

  MASK_PROJECT {
    string id PK "MASK-000127"
    uuid owner_id FK
    uuid part_id FK
    string status
    int current_revision
    string engine_version
    float fidelity
    string verdict
  }

  MASK_LAYER {
    uuid id PK
    string project_id FK
    int index "01..NN"
    string name
    string role "base|shadow|highlight|detail|stencil"
    bool is_physical
    float color_spec
  }

  GEOMETRY {
    uuid id PK
    uuid layer_id FK
    int schema_version
    float units_scale
    float bbox_w_mm
    float bbox_h_mm
  }

  GEOMETRY_PATH {
    uuid id PK
    uuid geometry_id FK
    string type "cut|score|crease|bridge"
    bool closed
    float length_mm
    float area_mm2
    int min_width_mm
  }

  VALIDATION_REPORT {
    uuid id PK
    string project_id FK
    uuid run_id FK
    string verdict
    int violations_total
    int auto_fixed
    tstz created_at
  }

  VIOLATION {
    uuid id PK
    uuid report_id FK
    string rule_code
    string severity "info|warning|error"
    float measured
    float required
    jsonb targets
    string message
  }

  MATERIAL_PROFILE {
    uuid id PK
    string code "matte-vinyl-090"
    string name
  }

  PLOTTER_PROFILE {
    uuid id PK
    string code "knife-25mm"
    string name
  }

  ARTIFACT {
    uuid id PK
    uuid run_id FK
    string kind "svg|dxf|plt|pdf|manifest|png"
    string storage_key
    string sha256
    float size_bytes
  }
```

## 3. Агрегаты и границы согласованности

| Агрегат | Граница | Транзакция |
|---|---|---|
| **MaskProject** | Проект целиком, включая слои и граф | Одна транзакция при ревизии |
| **Geometry** | Геометрия одного слоя | Отдельная транзакция (может быть большой) |
| **ValidationReport** | Отчёт + нарушения | Атомарно заменяет предыдущий |
| **EngineRun** | Статус прогона | Отдельные обновления статуса |
| **Order** | Заказ + позиции | Одна транзакция |

**Инварианты (нарушение = ошибка, не предупреждение):**

| # | Инвариант |
|---|---|
| I-1 | У каждой ревизии проекта есть хотя бы один слой |
| I-2 | Порядковые номера слоёв уникальны в пределах ревизии |
| I-3 | Layer Graph ацикличен по отношениям `under`/`over` |
| I-4 | Каждый слой ссылается на существующую геометрию |
| I-5 | Verdict `PASS` возможен только при `violations_error = 0` |
| I-6 | `Manifest` существует ⇔ есть хотя бы один артефакт |
| I-7 | Каждая ревизия ссылается на снимки профилей, использованные в прогоне |
| I-8 | Артефакты привязаны к `engine_version` |

## 4. Жизненный цикл Mask Project

```mermaid
stateDiagram-v2
  [*] --> Draft: создан
  Draft --> Processing: engine run запущен
  Processing --> Draft: нужен ручной input
  Processing --> NeedsWork: есть error-нарушения
  Processing --> NeedsReview: есть warning, errors нет
  NeedsWork --> Processing: auto-fix
  NeedsWork --> Draft: ручная правка в Studio
  NeedsReview --> Approved: подтверждён пользователем
  NeedsReview --> Draft: возврат на доработку
  Approved --> Ordered: заказ создан
  Approved --> Archived: без заказа
  Ordered --> InProduction: передан в производство
  InProduction --> Completed: маски вырезаны и нанесены
  InProduction --> Failed: физический брак
  Failed --> Draft: проект к доработке
  Completed --> Ordered: повторный заказ
  Archived --> [*]
  Completed --> [*]
```

| Статус | Что означает | Разрешено |
|---|---|---|
| `Draft` | Проект редактируется | редактирование |
| `Processing` | Идёт прогон ядра | только чтение |
| `NeedsWork` | Есть блокирующие нарушения | auto-fix, правка |
| `NeedsReview` | Производимо, требует подтверждения | подтверждение |
| `Approved` | Зафиксирован как производственный | только чтение + новый заказ |
| `Ordered` | Есть заказ | чтение |
| `InProduction` | Физическое изготовление | отчёт о физическом прогоне |
| `Completed` | Нанесено, результат подтверждён | повторный заказ |
| `Failed` | Физический брак | обязательная доработка проекта |

## 5. Ключевые идентификаторы

| Сущность | Формат | Пример | Правило |
|---|---|---|---|
| Mask Project | `MASK-` + 6 цифр | `MASK-000127` | Монотонный, не переиспользуемый |
| Layer | `L` + индекс + short-id | `L03-a91f` | Индекс уникален в ревизии |
| Geometry | UUIDv7 | `0193…` | Времяупорядоченный |
| Engine Run | `RUN-` + 6 цифр | `RUN-004512` | Монотонный |
| Rule code | `snake_case` | `min_line_width` | Стабильный, в документации |
| Profile code | `kebab-case` | `matte-vinyl-090` | Стабильный |
| Artifact kind | snake_case | `cut_svg` | Стабильный |

**Идентификатор проекта — навсегда.** Он печатается на физической маске
и в инструкции, является ключом повторного заказа.

## 6. Границы контекстов и кому что принадлежит

| Контекст | Владеет | Не владеет |
|---|---|---|
| **Mask** | Project, Layer, Geometry, Graph, Validation, Nesting | каталог объектов, заказы |
| **Supply** | MaterialProfile, PlotterProfile, RuleSet | проекты |
| **Catalog** | ObjectModel, Part, Surface, ForbiddenZone | геометрия проектов |
| **Commerce** | User, Order | геометрия, правила |
| **Core** | EngineRun, Artifact | всё остальное |

## 7. Соглашения об именовании в БД

* Таблицы: `snake_case` во множественном числе.
* PK: `uuid` (v7 для упорядоченной вставки).
* Внешние ключи: `<table>_id`.
* Время: `timestamptz`, всегда UTC.
* Денежные величины: минимальная единица (копейки/центы) как `integer`.
* Размеры: `real`/`numeric` в **миллиметрах**.
* Проценты/доли: `numeric`/`float` в диапазоне `0..1`.

## 8. Что сознательно не моделируется

| Не моделируем | Почему |
|---|---|
| «Красивый результат» | Нет измеримого представления; заменяется `fidelity` |
| Конкретный экземпляр детали | Дизайн отвязан от места нанесения (P-13) |
| Полная 3D-модель | До Stage 7 это данные каталога, а не домен Mask |
| История правок попиксельно | Достаточно ревизий проекта |
| Количество отходов материала | Полезно, но не влияет на доменную корректность |

## 9. Связанные документы

* `MASK_PROJECT.md` — структура и схема проекта.
* `LAYER_ENGINE.md`, `LAYER_GRAPH.md` — слой и граф.
* `VALIDATOR.md` — отчёт валидации.
* `../10_DATABASE/DATABASE_ARCHITECTURE.md` — физическая схема.