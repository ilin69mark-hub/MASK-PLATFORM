# LAYER_GRAPH

## 1. Зачем граф, а не список

Список слоёв подразумевает, что порядок — это просто последовательность.
Это верно только для непересекающихся элементов. Целевой класс графики —
переплетение линий, петли, ветвления, пересечения, вложенные контуры.

Примеры, где список не работает:

| Случай | Что ломается в списке |
|---|---|
| Линия A проходит **поверх** B, но B поверх C | Порядок противоречив в разных точках |
| Элемент D **пересекает** A, но должен быть **под** B | Нет способа выразить частичное отношение |
| Островок E пересекает B и требует **моста** к C | Нет структуры для хранения связей |
| Петля F **вложена** в A, но **выходит** из B | Простое сравнение индексов не работает |

Граф решает это явными отношениями и обнаружением циклов.

## 2. Типы отношений

```mermaid
flowchart TB
  A["Слой"] --> B["Слой"]
  A --> C["Слой"]
```

| Тип | Смысл | Направление | Влияет на порядок | Влияет на геометрию |
|---|---|---|---|---|
| `under` | A наносится под B | да | да | нет |
| `over` | A наносится поверх B | да | да | нет |
| `intersects` | геометрии пересекаются | нет | по контексту | да (нужен cutout) |
| `bridges` | A соединяет B мостами | да | да | да |
| `nested_in` | геометрия A внутри B | нет | нет | да (вложенность) |
| `masks` | A закрывает зону для B | да | да | да |
| `depends_on` | A не может существовать без B | да | да | возможно |
| `same_paint_op` | A и B в одной операции | нет | нет | да (объединяются) |

## 3. Модель графа

```mermaid
classDiagram
  class LayerGraph {
    +string version
    +list~string~ nodes
    +list~Edge~ edges
    +validate()
    +topological_order()
    +find_cycles()
    +resolve_regions()
  }
  class Edge {
    +string from
    +string to
    +EdgeType type
    +jsonb geometry_ref
    +float tolerance_mm
  }
  class LayerNode {
    +string layer_id
    +int index
    +string role
    +bbox bounds
  }
  LayerGraph "1" --> "*" LayerNode
  LayerGraph "1" --> "*" Edge
```

## 4. Пример графа реального проекта

```mermaid
graph TD
  L1["01 BASE BLACK"] -->|under| L2["02 BLACK SHADOW"]
  L2 -->|under| L3["03 RED BASE"]
  L3 -->|under| L4["04 DARK RED"]
  L4 -->|under| L5["05 WHITE"]
  L5 -->|under| L6["06 DETAIL"]

  L3 -.->|intersects| L4
  L6 -.->|bridges| L3
  L5 -.->|masks| L1
  L2 -.->|nested_in| L1

  classDef base fill:#1a1a1a,color:#fff,stroke:#666
  classDef mid fill:#4a4a52,color:#fff,stroke:#888
  classDef light fill:#e8e8ea,color:#000,stroke:#aaa
  classDef accent fill:#c41820,color:#fff,stroke:#a00
  classDef gold fill:#d6b026,color:#000,stroke:#aa8
  class L1 base
  class L2 mid
  class L3 accent
  class L4 mid
  class L5 light
  class L6 gold
```

## 5. Разрешение противоречий

### 5.1 Цикл в `under`/`over`

```mermaid
flowchart LR
  A["A under B"] --> C{"B under A?"}
  C -->|да| ERR["CYCLE<br/>order undefined"]
  C -->|нет| OK["порядок определён"]
```

Цикл в отношениях порядка — **фатальная ошибка**, не предупреждение.
Действие: попытаться разорвать цикл по силе отношения, иначе `FATAL`.

### 5.2 Частичные пересечения

Случай: A над B в области 1, но под B в области 2.

```mermaid
flowchart TB
  G["Глобальное отношение конфликтует"] --> R{"Геометрии реально<br/>пересекаются?"}
  R -->|нет| OK["Разделить на регионы,<br/>применить разный порядок"]
  R -->|да, в разных областях| SPLIT["Region-aware order<br/>две маски с cutout"]
  R -->|да, в одной области| F["Требуется 2 маски:<br/>A-mask и B-mask с вырезом"]
```

Решение: **геометрическое разрешение региона.** Слой разбивается на регионы
с разным порядком; в пересекающейся области создаются пары «маска + вырез».

### 5.3 Разрешение через приоритет роли

```mermaid
flowchart TB
  P["Конфликт порядка"] --> PRIO{"Роли известны?"}
  PRIO -->|да| RULE["Приоритет роли:<br/>detail > highlight > shadow > base"]
  PRIO -->|нет| ERR["Требуется ручное разрешение"]
```

Таблица приоритетов (детерминированное правило, не AI):

| Роль | Приоритет | Обоснование |
|---|---|---|
| `stencil` | 100 | закрывает зону, наносится последним поверх |
| `cutaway` | 95 | вырезает предыдущие слои |
| `detail` | 80 | тонкие элементы поверх |
| `highlight` | 60 | |
| `gradient` | 50 | |
| `shadow` | 40 | |
| `base` | 20 | фундамент |
| `shim` | 10 | подложка внизу |

## 6. Региональное разрешение (region-aware order)

```mermaid
flowchart LR
  subgraph GEOM["Geometry space"]
    R1["Region R1<br/>A over B"]
    R2["Region R2<br/>B over A"]
    R3["Region R3<br/>no overlap"]
  end
  R1 --> M1["Mask A∖R2"]
  R2 --> M2["Mask B∖R1"]
  R3 --> M3["Mask A∩R3 + B∩R3 (merged)"]
```

Алгоритм:

1. Разбить общую площадь на регионы по пересечению.
2. Для каждого региона определить локальный порядок.
3. Объединить регионы с одинаковым порядком в одну маску.
4. Проверить каждую результирующую маску на min width/gap.

## 7. Топологическая сортировка

```mermaid
flowchart TD
  A["edges: under/over/depends_on"] --> B["Build adjacency"]
  B --> C{"Cycle detected?"}
  C -->|да| D["Break by role priority<br/>log WARN"]
  C -->|нет| E["Kahn's algorithm<br/>deterministic tie-break by index"]
  D --> E
  E --> F["Assign physical indices 01..NN"]
  F --> G["Validate: order is total"]
```

Детерминированный tie-break: при равной «готовности» узлы сортируются
по `index`, затем по `layer_id`. Это гарантирует воспроизводимость (P-08).

## 8. Граф и физика: мосты

```mermaid
flowchart TB
  subgraph GEO["Geometry"]
    IS["Island<br/>area 0.42 mm²<br/>min_width 0.42 mm"]
    MAIN["Main film"]
    NB{"Needed?"}
  end
  IS --> NB
  NB -->|"min_island violated"| ERR["FAIL"]
  NB -->|"ok but bridge recommended"| BR["Add bridge → edge type: bridges"]
  BR --> MAIN
```

Тип отношения `bridges` в графе отражает **физическую связь** в геометрии:
мост — это path в слое-владельце, соединяющий островок с основной плёнкой.

## 9. Валидация графа

| Проверка | Severity | Сообщение |
|---|---|---|
| Ссылка на несуществующий слой | error | «Слой L07 упомянут в графе, но не существует» |
| Цикл в `under`/`over` | fatal | «Порядок слоёв не определён: A→B→C→A» |
| Один и тот же слой в `over` к двум слоям без приоритета | warning | «Неоднозначность порядка разрешена по приоритету роли» |
| Пересечение без отношения | warning | «L03 и L04 пересекаются, но отношение не задано» |
| Островок без моста при `bridges` ожидаемом | error | «Островок 3 в слое L03 не соединён» |
| Слой без входящих/исходящих рёбер | info | «Слой L01 изолирован» |

## 10. Визуализация в Studio

```mermaid
flowchart LR
  subgraph PANEL["Layer Panel"]
    L6["06 DETAIL"] --- L5["05 WHITE"]
    L5 --- L4["04 DARK RED"]
    L4 --- L3["03 RED BASE"]
    L3 --- L2["02 BLACK SHADOW"]
    L2 --- L1["01 BASE BLACK"]
  end
  L6 -.->|"bridges"| L3
  L5 -.->|"masks"| L1
```

UI показывает:
* порядок (сверху вниз = сверху вниз по нанесению);
* связи не-последовательных слоёв — тонкой линией с подписью типа;
* конфликты — красной рамкой и счётчиком на панели.

## 11. Влияние на физическое производство

Граф — не только UI-структура. Он напрямую определяет:

| Что | Как влияет граф |
|---|---|
| Порядок нанесения | Топологическая сортировка |
| Нужен ли cutout в нижнем слое | Отношения `over` + `intersects` |
| Где нужны мосты | Отношения `bridges` + результат Topology |
| Какие слои нельзя слить | Наличие циклов/конфликтов |
| Размер заказа | Число физических слоёв |
| Текст инструкции | Порядок шагов генерируется из графа |

## 12. Связанные документы

* `LAYER_ENGINE.md` — как рождаются физические слои.
* `../05_GEOMETRY/TOPOLOGY_ENGINE.md` — мосты и островки.
* `../07_STUDIO/MASK_STUDIO.md` — UI графа.
* `../06_MANUFACTURING/VALIDATOR.md` — проверки графа.