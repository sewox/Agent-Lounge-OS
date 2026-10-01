import type { Metadata } from "next";
import localFont from "next/font/local";
import { ApprovalAlertBridge } from "@/components/approval-alert-bridge";
import { ApprovalNotificationBridge } from "@/components/approval-notification-bridge";
import { AppShell } from "@/components/app-shell";
import { CommandPalette } from "@/components/command-palette";
import { LocaleProvider } from "@/components/locale-provider";
import { LoungeProvider } from "@/components/lounge-provider";
import { UiScaleProvider } from "@/components/ui-scale-provider";
import "./globals.css";

const geistSans = localFont({
  src: "./fonts/Geist-Variable.woff2",
  variable: "--font-geist-sans",
  weight: "100 900",
  display: "swap",
});

const ibmPlex = localFont({
  src: [
    {
      path: "./fonts/IBMPlexSans-Regular.woff2",
      weight: "400",
      style: "normal",
    },
    {
      path: "./fonts/IBMPlexSans-Medium.woff2",
      weight: "500",
      style: "normal",
    },
    {
      path: "./fonts/IBMPlexSans-SemiBold.woff2",
      weight: "600",
      style: "normal",
    },
  ],
  variable: "--font-ibm-plex",
  display: "swap",
});

const jetbrains = localFont({
  src: [
    {
      path: "./fonts/JetBrainsMono-Regular.woff2",
      weight: "400",
      style: "normal",
    },
    {
      path: "./fonts/JetBrainsMono-Medium.woff2",
      weight: "500",
      style: "normal",
    },
    {
      path: "./fonts/JetBrainsMono-SemiBold.woff2",
      weight: "600",
      style: "normal",
    },
    {
      path: "./fonts/JetBrainsMono-Bold.woff2",
      weight: "700",
      style: "normal",
    },
  ],
  variable: "--font-jetbrains",
  display: "swap",
});

const publicSans = localFont({
  src: [
    {
      path: "./fonts/PublicSans-Regular.woff2",
      weight: "400",
      style: "normal",
    },
    {
      path: "./fonts/PublicSans-Medium.woff2",
      weight: "500",
      style: "normal",
    },
    {
      path: "./fonts/PublicSans-SemiBold.woff2",
      weight: "600",
      style: "normal",
    },
  ],
  variable: "--font-public-sans",
  display: "swap",
});

export const metadata: Metadata = {
  title: "Agent Lounge OS",
  description: "Yerel ajan orkestrasyon katmanı",
  icons: {
    icon: [{ url: "/logo.png", type: "image/png" }],
    apple: "/logo.png",
  },
};

const localeBootstrapScript = `(function(){try{var k=localStorage.getItem("lounge.locale")||localStorage.getItem("locale");var lang="en";if(k&&String(k).toLowerCase().indexOf("tr")===0){lang="tr";}else if(!k&&typeof navigator!=="undefined"&&String(navigator.language||"").toLowerCase().indexOf("tr")===0){lang="tr";}document.documentElement.lang=lang;}catch(e){document.documentElement.lang="en";}})();`;

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html
      lang="en"
      suppressHydrationWarning
      className={`${geistSans.variable} ${ibmPlex.variable} ${jetbrains.variable} ${publicSans.variable} h-full antialiased`}
    >
      <head>
        <script dangerouslySetInnerHTML={{ __html: localeBootstrapScript }} />
      </head>
      <body className="h-full overflow-hidden bg-surface text-on-surface font-body">
        <UiScaleProvider>
          <LocaleProvider>
            <LoungeProvider>
              <ApprovalAlertBridge />
              <ApprovalNotificationBridge />
              <AppShell>{children}</AppShell>
              <CommandPalette />
            </LoungeProvider>
          </LocaleProvider>
        </UiScaleProvider>
      </body>
    </html>
  );
}
