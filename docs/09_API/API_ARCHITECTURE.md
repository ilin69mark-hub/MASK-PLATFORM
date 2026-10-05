# API_ARCHITECTURE

REST API на Go. API — тонкий слой: аутентификация, оркестрация, сериализация.
Вся логика предметной области — в Mask Engine.

## 1. Принципы

| Принцип | Реализация |
|---|---|
| REST, версия в URL | `/api/v1/…` |
| Ядро отделено от HTTP | Engine вызывается как библиотека или через очередь |
| Идемпотентность | `Idempotency-Key` для POST |
| Асинхронные тяжёлые операции | `202 Accepted` + статус прогона |
| Синхронно — только быстрое | Валидация, измерение, превью (< 2 s) |
| Ошибки — единый формат | См. раздел 6 |
| Пагинация курсорная | Не offset |
| Фильтрация — параметры | Не произвольные запросы |

## 2. Структура

```text
/api/v1
├── auth
│   ├── POST /register
│   ├── POST /login
│   ├── POST /logout
│   ├── POST /refresh
│   └── GET  /me
├── catalog
│   ├── GET  /brands
│   ├── GET  /models?brand=
│   ├── GET  /objects
│   ├── GET  /objects/{id}
│   ├── GET  /objects/{id}/parts
│   └── GET  /parts/{id}
├── projects
│   ├── POST   /
│   ├── GET    /
│   ├── GET    /{projectId}
│   ├── PATCH  /{projectId}
│   ├── DELETE /{projectId}
│   ├── POST   /{projectId}/revisions/{n}/restore
│   ├── GET    /{projectId}/revisions
│   ├── GET    /{projectId}/layers
│   ├── GET    /{projectId}/layers/{layerId}
│   ├── PATCH  /{projectId}/layers/{layerId}
│   ├── GET    /{projectId}/graph
│   ├── PUT    /{projectId}/graph
│   └── GET    /{projectId}/diff?from=1&to=2
├── engine
│   ├── POST   /projects/{projectId}/run
│   ├── POST   /projects/{projectId}/validate
│   ├── POST   /projects/{projectId}/autofix
│   ├── POST   /projects/{projectId}/simplify
│   ├── POST   /projects/{projectId}/nest
│   ├── GET    /projects/{projectId}/preview
│   └── GET    /runs/{runId}
├── profiles
│   ├── GET    /materials
│   ├── GET    /materials/{code}
│   ├── GET    /plotters
│   ├── GET    /cutting-profiles?material=&plotter=
│   └── POST   /cutting-profiles/preview
├── artifacts
│   ├── GET    /projects/{projectId}/artifacts
│   ├── GET    /projects/{projectId}/artifacts/{artifactId}
│   ├── POST   /projects/{projectId}/export
│   └── GET    /projects/{projectId}/export/download
├── orders
│   ├── POST   /
│   ├── GET    /
│   ├── GET    /{orderId}
│   ├── GET    /{orderId}/instructions
│   └── POST   /{orderId}/physical-proof
└── admin
    ├── GET    /runs
    ├── POST   /materials
    ├── PATCH  /materials/{code}/versions/{v}
    └── GET    /health
```

## 3. Ключевые эндпоинты

### 3.1 Создание проекта

```http
POST /api/v1/projects
Idempotency-Key: 01JB3X…
Content-Type: application/json
```

```json
{
  "name": "Hood graphic v1",
  "target": {
    "part_id": "pt_a4b9_hood"
  },
  "source": {
    "asset_id": "ast_01"
  },
  "manufacturing": {
    "material_code": "matte-vinyl-090",
    "material_version": 4,
    "plotter_code": "knife-25mm",
    "plotter_version": 2
  },
  "options": {
    "target_fidelity": 0.90,
    "simplify_strategy": "balanced",
    "autofix": true
  }
}
```

```http
201 Created
Location: /api/v1/projects/MASK-000127
```

```json
{
  "id": "MASK-000127",
  "status": "Draft",
  "revision": 1,
  "links": {
    "self": "/api/v1/projects/MASK-000127",
    "run": "/api/v1/engine/projects/MASK-000127/run",
    "layers": "/api/v1/projects/MASK-000127/layers"
  }
}
```

### 3.2 Запуск ядра

```http
POST /api/v1/engine/projects/MASK-000127/run
Idempotency-Key: 01JB3Y…
```

```json
{
  "target_fidelity": 0.90,
  "simplify_strategy": "balanced",
  "autofix": { "enabled": true, "max_iterations": 3 },
  "nest": { "enabled": true },
  "stage_filter": ["vectorize", "layerize", "topology", "validate"]
}
```

```http
202 Accepted
Retry-After: 5
```

```json
{
  "run_id": "RUN-004512",
  "status": "queued",
  "project_id": "MASK-000127",
  "poll": "/api/v1/engine/runs/RUN-004512",
  "stream": "/api/v1/engine/runs/RUN-004512/stream"
}
```

### 3.3 Результат прогона

```http
GET /api/v1/engine/runs/RUN-004512
```

```json
{
  "run_id": "RUN-004512",
  "project_id": "MASK-000127",
  "revision": 4,
  "status": "completed",
  "engine_version": "0.4.2",
  "started_at": "2026-01-15T16:02:45Z",
  "duration_ms": 2734,

  "stages": [
    { "stage": "vectorize", "status": "completed", "duration_ms": 1180 },
    { "stage": "layerize", "status": "completed", "duration_ms": 210 },
    { "stage": "topology", "status": "completed", "duration_ms": 402 },
    { "stage": "validate", "status": "completed", "duration_ms": 251 },
    { "stage": "autofix", "status": "completed", "duration_ms": 612, "iterations": 2 },
    { "stage": "nest", "status": "completed", "duration_ms": 71 }
  ],

  "result": {
    "verdict": "PASS_WITH_WARNINGS",
    "violations": { "error": 0, "warning": 2, "info": 5 },
    "fidelity": 0.912,
    "physical_layers": 6,
    "tiles": 30,
    "project_status": "NeedsReview"
  },

  "links": {
    "validation": "/api/v1/projects/MASK-000127/validation/latest",
    "artifacts": "/api/v1/projects/MASK-000127/artifacts",
    "preview": "/api/v1/engine/projects/MASK-000127/preview"
  }
}
```

### 3.4 Валидация (синхронная)

```http
POST /api/v1/engine/projects/MASK-000127/validate
```

```json
{
  "verdict": "FAIL",
  "counts": { "fatal": 0, "error": 9, "warning": 2, "info": 5 },
  "fidelity": 0.912,
  "violations": [
    {
      "rule_code": "min_line_width",
      "severity": "error",
      "layer": { "id": "L06-c90f", "index": 6, "name": "Detail" },
      "measurement": { "measured": 0.42, "required": 0.80, "unit": "mm", "operator": ">=" },
      "targets": { "count": 7, "path_ids": ["path_208", "path_213"] },
      "message": {
        "headline": "7 элементов слишком маленькие для выбранного материала.",
        "detail": "Минимум для matte-vinyl-090: 0.80 мм. Самый узкий элемент: 0.42 мм.",
        "reason": "Детализация ниже разрешающей способности материала.",
        "consequence": "При снятии маски эти элементы оторвутся и повредят деталь.",
        "actions": [
          { "id": "autofix", "label": "Исправить автоматически", "estimated_fidelity_loss": 0.02 },
          { "id": "show_elements", "label": "Показать элементы" },
          { "id": "change_material", "label": "Другой материал", "options": ["thin-vinyl-050"] }
        ]
      },
      "auto_fix": {
        "available": true,
        "operations": ["widen", "merge_nearby", "delete_micro"]
      }
    }
  ]
}
```

### 3.5 Auto-fix

```http
POST /api/v1/engine/projects/MASK-000127/autofix
```

```json
{
  "violation_ids": ["vio_004512_017", "vio_004512_018"],
  "preview": true,
  "max_fidelity_loss": 0.03
}
```

```json
{
  "preview": {
    "changes": [
      { "operation": "widen", "layer_id": "L06-c90f", "targets": 7, "delta_width_mm": 0.38 },
      { "operation": "add_bridge", "layer_id": "L03-0b83", "targets": 2 },
      { "operation": "merge_nearby", "layer_id": "L05-3e6d", "targets": 3 }
    ],
    "predicted_fidelity": 0.905,
    "fidelity_loss": 0.007,
    "predicted_verdict": "PASS"
  }
}
```

С `"preview": false` — применяет и создаёт новую ревизию.

### 3.6 Экспорт

```http
POST /api/v1/projects/MASK-000127/export
```

```json
{
  "formats": ["svg", "dxf", "plt", "pdf", "manifest"],
  "include_nesting": true,
  "archive": true
}
```

```http
202 Accepted
```

```json
{
  "export_id": "EXP-004512",
  "status": "queued",
  "download_url": "/api/v1/projects/MASK-000127/export/download?rev=4"
}
```

### 3.7 Повторное производство

```http
POST /api/v1/projects/{id}/reproduce
```

```json
{
  "target_revision": 4,
  "mode": "regenerate"
}
```

| `mode` | Что делает |
|---|---|
| `regenerate` | Перегенерировать артефакты из данных проекта |
| `archive` | Отдать уже сохранённые артефакты (быстрее) |
| `verify` | Перегенерировать и сравнить хеши (аудит воспроизводимости) |

```json
{
  "mode": "verify",
  "revision": 4,
  "engine_version": "0.4.2",
  "artifacts_checked": 42,
  "identical": 41,
  "differs": [
    { "file": "L06_A1.svg", "reason": "engine_version changed: 0.4.2 → 0.5.0" }
  ],
  "reproducible_with_current_engine": true
}
```

## 4. Синхронно vs асинхронно

| Операция | Режим | Бюджет |
|---|---|---|
| Валидация одного слоя | sync | 300 ms |
| Полная валидация | sync до 2 s, иначе async | 2 s |
| Auto-fix превью | sync | 1 s |
| Auto-fix применение | async | 30 s |
| Прогон ядра целиком | async | 60 s |
| Экспорт | async | 60 s |
| Превью рендер | sync | 800 ms |
| Инструкция | async | 20 s |

## 5. Аутентификация

| Аспект | Реализация |
|---|---|
| Схема | Bearer JWT (RS256) |
| Access token | 15 min |
| Refresh token | 30 дней, ротация |
| Хранение | httpOnly cookie для refresh, memory для access |
| Роли | `user`, `operator`, `admin` |
| Авторизация | По владельцу проекта; `operator` — доступ к производственным операциям |

| Роль | Права |
|---|---|
| `user` | Свои проекты, каталог, экспорт |
| `operator` | + физические прогоны, подтверждение проектов |
| `admin` | + профили материалов, каталог объектов |

## 6. Формат ошибок

```json
{
  "error": {
    "code": "PROJECT_NOT_MANUFACTURABLE",
    "message": "Экспорт недоступен: проект не прошёл валидацию.",
    "detail": "Найдено 9 блокирующих нарушений. Исправьте их или примените авто-фикс.",
    "status": 409,
    "violations_url": "/api/v1/engine/projects/MASK-000127/validate",
    "actions": [
      { "id": "run_autofix", "label": "Исправить автоматически" }
    ],
    "trace_id": "01JB3YQ…"
  }
}
```

### Коды ошибок

| Код | HTTP | Смысл |
|---|---|---|
| `PROJECT_NOT_FOUND` | 404 | |
| `PROJECT_NOT_MANUFACTURABLE` | 409 | есть `error`-нарушения |
| `INVALID_PROJECT_STATE` | 409 | операция недопустима в текущем статусе |
| `LAYER_CYCLE_DETECTED` | 422 | цикл в графе слоёв |
| `GEOMETRY_INVALID` | 422 | NaN/Inf/вырожденные контуры |
| `PROFILE_NOT_FOUND` | 404 | |
| `PROFILE_NOT_VERIFIED` | 409 | профиль в статусе draft |
| `RUN_IN_PROGRESS` | 409 | уже выполняется прогон |
| `LIMIT_EXCEEDED` | 413 | превышен лимит размера/слоёв |
| `RATE_LIMITED` | 429 | |
| `ENGINE_ERROR` | 500 | внутренняя ошибка ядра |

## 7. Идемпотентность

```http
POST /api/v1/engine/projects/MASK-000127/run
Idempotency-Key: 01JB3Y…
```

| Правило | Значение |
|---|---|
| Срок хранения ключа | 24 часа |
| Повтор с тем же ключом | Возвращается тот же `run_id`, не новый |
| Повтор с другим телом | `409 IDEMPOTENCY_KEY_REUSED` |
| Обязательно для | `run`, `autofix` (без preview), `export`, `create project`, `order` |

## 8. Ограничения и пагинация

```http
GET /api/v1/projects?status=NeedsReview&material=matte-vinyl-090&limit=50&cursor=eyJ0cyI6…
```

```json
{
  "data": [ /* … */ ],
  "page": {
    "next_cursor": "eyJ0cyI6MTcyNjAwMDAwMDB9",
    "has_more": true,
    "limit": 50
  }
}
```

| Ограничение | Значение |
|---|---|
| Макс. `limit` | 200 |
| Размер тела запроса | 16 MB |
| Загрузка файлов | через presigned URL в S3 |
| Макс. растровый файл | 40 Мпикс |

## 9. Потоковая доставка результатов

```http
GET /api/v1/engine/runs/RUN-004512/stream
Accept: text/event-stream
```

```text
event: stage
data: {"stage":"vectorize","status":"completed","duration_ms":1180}

event: stage
data: {"stage":"layerize","status":"completed","duration_ms":210}

event: violation
data: {"rule_code":"min_line_width","severity":"error","count":7}

event: done
data: {"status":"completed","verdict":"FAIL","violations":{"error":9,"warning":2,"info":5}}
```

## 10. Версионирование API

| Правило | Значение |
|---|---|
| Версия в URL | `/api/v1` |
| Изменение | Только обратно совместимое в рамках версии |
| Удаление поля | Только в новой версии |
| Deprecated | Заголовок `Deprecation` + срок |
| Новые обязательные поля | Новая версия |

## 11. Контракты

| Тип | Формат | Хранение |
|---|---|---|
| OpenAPI 3.1 | `openapi.yaml` | В репозитории, генерируется из кода |
| JSON Schema для Mask Project | `schema/mask-project.v1.json` | В репозитории |
| JSON Schema для Manifest | `schema/manifest.v1.json` | В репозитории |
| Примеры | `api/examples/*.json` | В репозитории |

**CI проверяет:** соответствие OpenAPI фактическим роутам, валидность примеров,
совместимость схем.

## 12. Связанные документы

* `../02_ARCHITECTURE/SYSTEM_ARCHITECTURE.md` — где находится API.
* `../10_DATABASE/DATABASE_ARCHITECTURE.md` — что за этими эндпоинтами.
* `../06_MANUFACTURING/EXPORT_ENGINE.md` — формат отдаваемых файлов.
* `../04_ENGINE/MASK_ENGINE.md` — что вызывается.