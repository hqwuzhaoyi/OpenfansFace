import { PersonWorkbench } from '@/components/persons/workbench'

export default async function PersonPage({params}:{params:Promise<{name:string}>}) {
  const {name}=await params
  return <PersonWorkbench initialName={name}/>
}
