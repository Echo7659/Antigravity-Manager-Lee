import { createElement, type ReactNode } from 'react';
import { Bot, Sparkles } from 'lucide-react';
import { MODEL_CONFIG, compareModelsDesc, inferModelGroup } from '../config/modelConfig';

export interface ProxyModel {
    id: string;
    name: string;
    desc: string;
    group: string;
    icon: ReactNode;
}

export function buildProxyModels(ids: string[]): ProxyModel[] {
    return [...new Set(ids)].map(id => {
        const group = inferModelGroup(id);
        const Icon = MODEL_CONFIG[id.toLowerCase()]?.Icon;
        const icon = Icon
            ? createElement(Icon, { size: 16 })
            : group === 'Claude'
                ? createElement(Sparkles, { size: 16, className: 'text-purple-400' })
                : createElement(Bot, { size: 16, className: 'text-blue-400' });

        return { id, name: id, desc: id, group, icon };
    }).sort(compareModelsDesc);
}
