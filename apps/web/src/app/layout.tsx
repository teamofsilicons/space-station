// The document every route shares: Silicon UI's tokens, the station's styles, and the colour scheme
// chosen before the first paint.
import type { Metadata, Viewport } from "next";
import { THEME_SCRIPT } from "../theme-script";
import "@/components/silicon-ui/foundation.css";
import "../style.css";

export const metadata: Metadata = {
  title: "Space Station",
  description: "Records, live views, and notifications for carbons and silicons.",
  icons: { icon: { url: "/brand/mark.svg", type: "image/svg+xml" } },
};
export const viewport: Viewport = { themeColor: "#f7f8fa" };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" data-accent="blue" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_SCRIPT }} />
      </head>
      <body>{children}</body>
    </html>
  );
}
