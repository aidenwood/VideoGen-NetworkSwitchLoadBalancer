import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import './styles.css';

/* StrictMode double-invokes effects in dev, which would double the 2s poll while
   developing. That's deliberate — it catches an effect that isn't idempotent,
   and every poll here is. */
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
