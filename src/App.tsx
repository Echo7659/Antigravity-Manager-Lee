import { createBrowserRouter, RouterProvider } from 'react-router-dom';

import Layout from './components/layout/Layout';
import Dashboard from './pages/Dashboard';
import Accounts from './pages/Accounts';
import Settings from './pages/Settings';
import ApiProxy from './pages/ApiProxy';
import Monitor from './pages/Monitor';
import TokenStats from './pages/TokenStats';
import Security from './pages/Security';
import ThemeManager from './components/common/ThemeManager';
import UserToken from './pages/UserToken';
import SuggestionDeleteThinkingModal from './components/common/SuggestionDeleteThinkingModal';
import DebugConsole from './components/debug/DebugConsole';
import { useEffect, startTransition } from 'react';
import { useConfigStore } from './stores/useConfigStore';
import { useTranslation } from 'react-i18next';
import { AdminAuthGuard } from './components/common/AdminAuthGuard';

const router = createBrowserRouter([
  {
    path: '/',
    element: <Layout />,
    children: [
      {
        index: true,
        element: <Dashboard />,
      },
      {
        path: 'accounts',
        element: <Accounts />,
      },
      {
        path: 'api-proxy',
        element: <ApiProxy />,
      },
      {
        path: 'monitor',
        element: <Monitor />,
      },
      {
        path: 'token-stats',
        element: <TokenStats />,
      },
      {
        path: 'user-token',
        element: <UserToken />,
      },
      {
        path: 'security',
        element: <Security />,
      },
      {
        path: 'settings',
        element: <Settings />,
      },
    ],
  },
]);

function App() {
  const { config, loadConfig } = useConfigStore();
  const { i18n } = useTranslation();

  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  // Sync language from config (仅在不同步时通过 startTransition 非阻塞调度)
  useEffect(() => {
    if (config?.language && i18n.language !== config.language) {
      startTransition(() => {
        i18n.changeLanguage(config.language);
      });
      document.documentElement.dir = config.language === 'ar' ? 'rtl' : 'ltr';
    }
  }, [config?.language, i18n]);

  return (
    <AdminAuthGuard>
      <ThemeManager />
      <DebugConsole />
      <SuggestionDeleteThinkingModal />
      <RouterProvider router={router} />
    </AdminAuthGuard>
  );
}

export default App;
