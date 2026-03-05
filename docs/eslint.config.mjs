/* docs/eslint.config.mjs */

import { defineConfig, globalIgnores } from 'eslint/config'
import nextVitals from 'eslint-config-next/core-web-vitals'
import oxlint from 'eslint-plugin-oxlint'

const eslintConfig = defineConfig([
	...nextVitals,
	globalIgnores(['.next/**', 'out/**', 'build/**', 'next-env.d.ts', '.source/**']),
	// Must be last: disables ESLint rules already covered by oxlint
	oxlint.configs['flat/recommended'],
])

export default eslintConfig
