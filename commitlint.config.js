export default {
	extends: ['@commitlint/config-conventional'],
	plugins: [
		{
			rules: {
				'subject-starts-lower': ({ subject }) => [
					!subject || /^[a-z]/.test(subject),
					'subject must start with a lower-case letter'
				]
			}
		}
	],
	rules: {
		'header-max-length': [2, 'always', 72],
		'subject-case': [0],
		'subject-starts-lower': [2, 'always']
	}
};
