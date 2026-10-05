# SYSTEM_ARCHITECTURE

## 1. Общая схема

```mermaid
flowchart TB
  subgraph CLIENT["Client"]
    direction LR
    WEB["Web Platform<br/>Next.js"]
    STUDIO["Mask Studio<br/>Next.js"]
    WASM["Browser Engine<br/>Rust → WASM"]
  end

  subgraph EDGE["API Layer · Go"]
    API["REST API"]
    AUTH["Auth / Sessions"]
    ORCH["Job Orchestrator"]
  end

  subgraph CORE["Mask Engine · Rust (core)"]
    direction TB
    IMG["Image Processor"]
    VEC["Vector Engine"]
    LAY["Layer Engine + Graph"]
    TOPO["Topology Engine"]
    RULES["Rules Engine"]
    VAL["Validator"]
    FIX["Auto-Fix / Simplify"]
    NEST["Nesting Engine"]
    EXP["Export Engine"]
    INST["Instruction Engine"]
  end

  subgraph SVC["Services · Go"]
    PROJ["Project Service"]
    CAT["Catalog Service"]
    ORD["Order Service"]
  end

  subgraph DATA["Data"]
    PG[("PostgreSQL")]
    RD[("Redis")]
    S3[("S3 Storage")]
  end

  WEB --> API
  STUDIO --> API
  STUDIO -.->|"локальный preview"| WASM
  API --> CORE
  API --> SVC
  API --> PG
  API --> RD
  SVC --> PG
  CORE --> S3
  SVC --> S3
  CORE -.->|"тот же бинарник"| WASM
```

Ключевое: `CORE` и `WASM` — **один и тот же код**, собранный дважды.
Это возможно только при условии, что ядро не зависит от ОС, сети и файловой системы.

## 2. Слои системы

| Слой | Ответственность | Технологии | Зависит от |
|---|---|---|---|
| **L1 Kernel** | Геометрия, топология, offset, пересечения | Rust | ничего |
| **L2 Engine** | Конвейер: image → layer → rules → validate → export | Rust + L1 | L1 |
| **L3 Services** | Проекты, каталог, заказы, пользователи | Go | БД |
| **L4 API** | HTTP, авторизация, оркестрация | Go | L2, L3 |
| **L5 Clients** | Web, Studio, WASM-preview | Next.js, WASM | L4 (L1 опционально) |

Правило зависимостей: **вверх по строкам можно, вниз нельзя.** L1 не знает про L3.

## 3. Компоненты и их границы

```mermaid
flowchart LR
  subgraph L1["L1 Kernel (чистая геометрия, без I/O)"]
    G1[Primitives<br/>Point/Segment/Path/Contour]
    G2[Boolean ops<br/>union/diff/intersect]
    G3[Offset<br/>grow/shrink]
    G4[Measure<br/>area/length/width]
    G5[Warp<br/>affine/projective]
    G6[Flatten<br/>curve→polyline]
  end
  subgraph L2["L2 Engine"]
    E1[Image Processor]
    E2[Vector Engine]
    E3[Layer Engine]
    E4[Topology Engine]
    E5[Rules Engine]
    E6[Validator]
    E7[Auto-Fix]
    E8[Nesting Engine]
    E9[Export Engine]
    E10[Instruction Engine]
  end
  E2 --> G1
  E4 --> G2
  E4 --> G3
  E4 --> G4
  E6 --> G5
  E8 --> G4
```

### Контракт L1 → L2

L1 — чистые функции над структурами данных. Требования:

* никаких `Result`-обёрток с доменными ошибками (доменные ошибки — в L2);
* никакого доступа к файлам, сети, БД, времени, окружению;
* детерминированность: один вход → один выход, без плавающего порядка обхода;
* единицы измерения фиксированы (см. `05_GEOMETRY/TOPOLOGY_ENGINE.md`, ADR-0004).

## 4. Почему Rust в ядре, Go в сервисах

Это **не «модный стек»**, а разделение по требованиям.

| Требование | Rust | Go |
|---|---|---|
| Сложная геометрия, offset, boolean-операции | ✅ высокая производительность, строгая память, предсказуемое время | ⚠️ GC-паузы, отсутствие готовых robust-примитивов |
| Компиляция в WASM | ✅ первоклассная цель | ❌ не практично |
| Массовая параллельность воркеров | ✅ rayon, без GC | ✅ тоже хорошо |
| HTTP, ORM, бизнес-логика, оркестрация | ⚠️ медленнее разработка | ✅ стандарт индустрии |
| Скорость разработки CRUD-сервисов | ⚠️ | ✅ |
| Экосистема геометрии (clipper2, i_overlay, geo) | ✅ сильная | ❌ |

Вывод: **вычислительное ядро** — Rust; **системный слой** — Go. Разные профили нагрузки.

## 5. Модель исполнения задач

```mermaid
sequenceDiagram
  participant C as Client
  participant A as API (Go)
  participant Q as Redis Queue
  participant W as Worker (Go + Rust)
  participant S as S3

  C->>A: POST /projects/{id}/run
  A->>A: сохранить EngineRun (status=queued)
  A->>Q: enqueue(run_id, engine_version)
  A-->>C: 202 { run_id, status: queued }
  W->>Q: dequeue
  W->>W: rust core: image → validate
  W->>S: записать артефакты + manifest
  W->>S: записать отчёт валидации
  W->>Q: publish run.completed
  A->>C: SSE/WebSocket run.completed
```

Синхронный (интерактивный) путь для быстрых операций: `validate`, `measure`,
`preview` — выполняются в том же процессе API, без очереди (budget < 2 s).

## 6. Browser Engine (WASM)

```mermaid
flowchart LR
  subgraph BROWSER
    UI[Studio UI]
    W[wasm-pack module]
    M[(in-memory MaskProject)]
  end
  UI -->|user edit| M
  M -->|validate/preview/nest| W
  W -->|diagnostics| UI
  W -->|snapshots| M
  M -->|сохранить| API
```

**Что обязано работать в браузере:** геометрия, топология, валидация, live-preview,
nest-превью. Узкое место — взаимодействие с Worker API для длительных операций.

**Что не работает в браузере:** обучение моделей, тяжёлый image processing,
экспорт в PLT/PDF (делается на сервере).

## 7. Потоки данных

```mermaid
flowchart TD
  U1[Пользователь: PNG] --> OBJ[(object storage: source/)]
  OBJ --> ENG[Engine Run]
  ENG --> GEO[geometry store: JSONB + protobuf]
  GEO --> VAL[validation report]
  VAL --> ART[artifacts/: run_id/]
  ENG --> PRE[previews/: run_id/]
  ENG --> MAN[manifest.json]
  PROJ[(projects table)] --- GEO
  PROJ --- VAL
```

## 8. Хранилища и их границы

| Хранилище | Что хранит | Почему там |
|---|---|---|
| **PostgreSQL** | Проекты, метаданные, слои, версии, валидации, каталог, пользователи | Транзакции, связи, версии |
| **PostgreSQL (JSONB)** | Геометрия слоёв, layer graph, снимки профилей | Гибкая схема + GIN-индексы |
| **Redis** | Очереди, кэш прогонов, idempotency keys, rate limit | Быстро, эфемерно |
| **S3** | Исходники, артефакты, превью, манифесты, инструкции | Большие объекты, версионирование |
| **ClickHouse (P2)** | Метрики прогонов и телеметрия геометрии | Аналитика больших объёмов |

Бинарная геометрия (сырые point-буферы) при необходимости выносится в S3,
в БД остаётся индекс и хеш.

## 9. Наблюдаемость

Три уровня телеметрии, все через OpenTelemetry:

| Уровень | Что измеряем |
|---|---|
| **Span** | Каждая стадия Engine Run: image_process, vectorize, layerize, topo, validate, fix, nest, export |
| **Metric** | `engine_stage_duration_ms`, `violations_count`, `fidelity`, `auto_fix_applied`, `paths_per_project` |
| **Log** | Структурированный, `project_id`, `run_id`, `engine_version`, `stage` |

**Обязательные алерты:**
* p95 полного прогона > целевого бюджета;
* доля `FAIL` валидаций > 30% на типовом входе;
* любая паника ядра (паника = дефект, не «обработать»).

## 10. Ограничения и явные «нет»

| Решение | Почему «нет» |
|---|---|
| Микросервисы на каждой сущности | Один разработчик, ранняя стадия. Модульный монолит + один worker |
| Kafka | Redis Streams достаточно |
| Kubernetes сразу | Docker Compose + один хост до реальной нагрузки |
| Event Sourcing | Версионирование проекта даёт нужную воспроизводимость проще |
| GraphQL | REST достаточно; контракт проще стабилизировать |
| Отдельный сервис превью | Превью — функция ядра |

## 11. Эволюция архитектуры

```mermaid
flowchart LR
  A["S1-S2<br/>Rust kernel CLI<br/>модульный монолит Go"] --> B["S3-S4<br/>+ воркеры<br/>+ WASM preview"]
  B --> C["S5-S7<br/>выделение Catalog<br/>+ Object Fit service"]
  C --> D["S8+<br/>+ Marketplace service<br/>+ аналитика"]
```

Каждое выделение сервиса требует ADR и реальной причины (границы владения данными,
изолированная нагрузка, независимый релиз).

## 12. Связанные документы

* `MASK_ENGINE.md` — внутреннее устройство ядра.
* `DATABASE_ARCHITECTURE.md` — схема хранения.
* `API_ARCHITECTURE.md` — контракты.
* `INFRASTRUCTURE.md` — окружения и развёртывание.