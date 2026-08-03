/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./src/**/*.{html,js}",
    "../src/dashboard.rs"
  ],
  theme: {
    extend: {
      colors: {
        brand: {
          50: '#f0f9ff',
          500: '#06b6d4',
          600: '#0891b2',
          900: '#164e63',
        }
      }
    },
  },
  plugins: [],
}
