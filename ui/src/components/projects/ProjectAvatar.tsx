import { Sparkles } from 'lucide-react'

import { cn } from '@/lib/utils'

import { projectLogoSrc } from './shared'

interface ProjectAvatarProps {
  projectName: string
  className?: string
  iconClassName?: string
}

export function ProjectAvatar({ projectName, className, iconClassName }: ProjectAvatarProps) {
  return (
    <div className={cn('relative shrink-0', className)}>
      <img
        src={projectLogoSrc(projectName)}
        alt=""
        className="h-full w-full rounded-[inherit] border border-border/60 bg-white/85 object-contain p-1 shadow-inner"
        onError={(event) => {
          const image = event.currentTarget
          image.style.display = 'none'
          image.nextElementSibling?.classList.remove('hidden')
        }}
      />
      <Sparkles
        className={cn(
          'hidden h-full w-full rounded-[inherit] border border-border/60 bg-muted/60 p-2 text-muted-foreground',
          iconClassName,
        )}
      />
    </div>
  )
}
