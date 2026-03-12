import { useEffect, useState, useTransition } from "react"

import { useSeamData } from "@canmi/seam-react"

import { createSeamClient, type ListConnectionsOutput } from "../../.seam/generated/client"

type Connection = ListConnectionsOutput["connections"][number]

interface PageData extends Record<string, unknown> {
  connections: ListConnectionsOutput
}

function renderStartedAt(value: string) {
  const unixMs = Number(value)
  if (!Number.isFinite(unixMs)) return value
  return new Date(unixMs).toLocaleString()
}

function InteractiveConnectionRows({ connections }: { connections: Connection[] }) {
  return (
    <>
      {connections.map((connection) => (
        <tr key={connection.id}>
          <td>
            <span className="cell-main">{connection.id}</span>
            <span className="cell-sub">{connection.peer_addr}</span>
          </td>
          <td>
            <span className="pill" data-layer={connection.layer}>
              {connection.layer.toUpperCase()}
            </span>
          </td>
          <td>
            <div className="meta-grid">
              <span className="cell-main">{connection.phase}</span>
              <span className="cell-sub">Port {connection.listen_port}</span>
            </div>
          </td>
          <td>
            <div className="meta-grid">
              <span className="cell-main">{connection.server_addr}</span>
              <span className="cell-sub">{connection.forward_target ?? "No upstream target"}</span>
            </div>
          </td>
          <td>{renderStartedAt(connection.started_at_unix_ms)}</td>
        </tr>
      ))}
    </>
  )
}

export default function ConnectionListPage() {
  const data = useSeamData<PageData>()
  const [mounted, setMounted] = useState(false)
  const [pending, startTransition] = useTransition()
  const [connections, setConnections] = useState(data.connections.connections)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setMounted(true)
  }, [])

  useEffect(() => {
    setConnections(data.connections.connections)
  }, [data.connections.connections])

  function refreshConnections() {
    startTransition(() => {
      void createSeamClient(window.location.origin)
        .listConnections({})
        .then((next) => {
          setConnections(next.connections)
          setError(null)
        })
        .catch((nextError) => {
          setError(nextError instanceof Error ? nextError.message : String(nextError))
        })
    })
  }

  if (!mounted) {
    return (
      <>
        <section className="hero">
          <div>
            <p className="eyebrow">Control Plane</p>
            <h1 className="title">Vane Console</h1>
            <p className="subtitle">
              Live visibility into the current proxy session set. The first paint is rendered from
              display-ready connection data, then hydration enables ad hoc refresh.
            </p>
          </div>
          <div className="hero-stat">
            <span className="hero-stat-label">Active connections</span>
            <span className="hero-stat-value">{data.connections.total}</span>
          </div>
        </section>

        <section className="panel">
          <div className="panel-header">
            <div>
              <h2 className="panel-title">Connection registry</h2>
              <p className="panel-copy">Current active sessions across all configured listeners.</p>
            </div>
            <div className="toolbar">
              <button className="refresh-button" disabled type="button">
                Refresh
              </button>
            </div>
          </div>

          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Connection</th>
                  <th>Layer</th>
                  <th>Stage</th>
                  <th>Route</th>
                  <th>Started</th>
                </tr>
              </thead>
              <tbody>
                {data.connections.connections.map((connection) => (
                  <tr key={connection.id}>
                    <td>
                      <span className="cell-main">{connection.id}</span>
                      <span className="cell-sub">{connection.peer_addr}</span>
                    </td>
                    <td>
                      <span className="pill" data-layer={connection.layer}>
                        {connection.layer}
                      </span>
                    </td>
                    <td>
                      <div className="meta-grid">
                        <span className="cell-main">{connection.phase}</span>
                        <span className="cell-sub">Port {connection.listen_port}</span>
                      </div>
                    </td>
                    <td>
                      <div className="meta-grid">
                        <span className="cell-main">{connection.server_addr}</span>
                        <span className="cell-sub">
                          {connection.forward_target ?? "No upstream target"}
                        </span>
                      </div>
                    </td>
                    <td>{connection.started_at_unix_ms}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      </>
    )
  }

  return (
    <>
      <section className="hero">
        <div>
          <p className="eyebrow">Control Plane</p>
          <h1 className="title">Vane Console</h1>
          <p className="subtitle">
            Live visibility into the current proxy session set. The first paint is rendered from
            display-ready connection data, then hydration enables ad hoc refresh.
          </p>
        </div>
        <div className="hero-stat">
          <span className="hero-stat-label">Active connections</span>
          <span className="hero-stat-value">{connections.length}</span>
        </div>
      </section>

      <section className="panel">
        <div className="panel-header">
          <div>
            <h2 className="panel-title">Connection registry</h2>
            <p className="panel-copy">Current active sessions across all configured listeners.</p>
          </div>
          <div className="toolbar">
            <button
              className="refresh-button"
              disabled={!mounted || pending}
              onClick={refreshConnections}
              type="button"
            >
              {pending ? "Refreshing..." : "Refresh"}
            </button>
          </div>
        </div>

        {error ? <p className="error-banner">{error}</p> : null}
        {connections.length === 0 ? (
          <p className="empty-state">No active connections</p>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Connection</th>
                  <th>Layer</th>
                  <th>Stage</th>
                  <th>Route</th>
                  <th>Started</th>
                </tr>
              </thead>
              <tbody>
                <InteractiveConnectionRows connections={connections} />
              </tbody>
            </table>
          </div>
        )}
      </section>
    </>
  )
}
