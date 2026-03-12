import type { ReactNode } from "react"

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <div className="shell">
      <div className="frame">{children}</div>
    </div>
  )
}
