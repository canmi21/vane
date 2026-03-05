/* docs/app/(home)/page.tsx */

'use client'

export default function HomePage() {
  const goToDocs = () => {
    window.location.href = '/docs'
  }
  return (
    <div className="flex flex-col justify-center text-center flex-1">
      <h1 className="text-2xl font-bold mb-4">Hello World</h1>
      <p>
        You can open{' '}
        <button
          onClick={goToDocs}
          className="font-medium underline"
          style={{ background: 'none', border: 'none', padding: 0, cursor: 'pointer' }}
        >
          /docs
        </button>{' '}
        and see the documentation.
      </p>
    </div>
  )
}
