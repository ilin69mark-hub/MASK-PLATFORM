import Link from "next/link";

export default function Home() {
  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: "3rem", maxWidth: "48rem" }}>
      <h1>MASK PLATFORM</h1>
      <p style={{ lineHeight: 1.6, color: "#333" }}>
        Изображение превращается в многослойный проект физической маски, который
        можно вырезать, нанести и воспроизвести через год на новом экземпляре
        той же детали.
      </p>
      <p>
        <Link href="/studio">Открыть Mask Studio →</Link>
      </p>
      <h2 style={{ fontSize: "1rem", marginTop: "2.5rem" }}>Что уже работает</h2>
      <ul style={{ lineHeight: 1.7 }}>
        <li>Геометрическое ядро на Rust, измеряет и проверяет производимость.</li>
        <li>То же ядро в браузере через WebAssembly — числа совпадают с серверными.</li>
        <li>Экспорт в SVG, DXF, HPGL и манифест производства.</li>
        <li>Разбиение маски на плитки с метками совмещения.</li>
      </ul>
      <p style={{ color: "#666", marginTop: "2rem", fontSize: "0.9rem" }}>
        Пороги производства ещё не подтверждены вырезом реального материала.
        Проходящая проверка означает соответствие правилам, а не физическую
        пригодность.
      </p>
    </main>
  );
}
