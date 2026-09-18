import type { Metadata } from "next";
import { Geist, IBM_Plex_Sans, JetBrains_Mono, Public_Sans } from "next/font/google";
import { AppShell } from "@/components/app-shell";
import { LoungeProvider } from "@/components/lounge-provider";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin", "latin-ext"],
});

const ibmPlex = IBM_Plex_Sans({
  variable: "--font-ibm-plex",
  subsets: ["latin", "latin-ext"],
  weight: ["400", "500", "600"],
});

const jetbrains = JetBrains_Mono({
  variable: "--font-jetbrains",
  subsets: ["latin", "latin-ext"],
  weight: ["400", "500", "600", "700"],
});

const publicSans = Public_Sans({
  variable: "--font-public-sans",
  subsets: ["latin", "latin-ext"],
  weight: ["400", "500", "600"],
});

export const metadata: Metadata = {
  title: "Agent Lounge OS",
  description: "Yerel ajan orkestrasyon katmanı",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html
      lang="tr"
      className={`${geistSans.variable} ${ibmPlex.variable} ${jetbrains.variable} ${publicSans.variable} h-full antialiased`}
    >
      <body className="min-h-full flex flex-col bg-surface text-on-surface font-body">
        <LoungeProvider>
          <AppShell>{children}</AppShell>
        </LoungeProvider>
      </body>
    </html>
  );
}
