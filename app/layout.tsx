/* app/layout.tsx */

import { Inter } from 'next/font/google'
import Script from 'next/script'
import { Provider } from '@/components/provider'
import './style.css'

const inter = Inter({
  subsets: ['latin'],
})

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html lang="en" className={inter.className} suppressHydrationWarning>
      <head>
        <Script
          src="https://www.googletagmanager.com/gtag/js?id=G-7QHHNT4WWH"
          strategy="afterInteractive"
        />
        <Script id="google-analytics" strategy="afterInteractive">
          {`
						window.dataLayer = window.dataLayer || [];
						function gtag(){dataLayer.push(arguments);}
						gtag('js', new Date());
						gtag('config', 'G-7QHHNT4WWH');
					`}
        </Script>
      </head>
      <body className="flex flex-col min-h-screen">
        <Provider>{children}</Provider>
      </body>
    </html>
  )
}
