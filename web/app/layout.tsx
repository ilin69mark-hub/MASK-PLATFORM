import type { Metadata } from "next";

import "./studio.css";

export const metadata: Metadata = {
  title: "MASK PLATFORM",
  description: "Production of multilayer physical masks for airbrushing.",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ru">
      <body style={{ margin: 0 }}>{children}</body>
    </html>
  );
}
