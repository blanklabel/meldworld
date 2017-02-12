package model

type WorldMap struct {
	ModelType
	MapObj
	Entities []Entity
}

// Size of a map
type Dimension struct {
	Height int
	Width  int
}

type Climate struct {
	//TODO: Come up with some base map features
}

type Props struct {
	Collision      bool `json:"collision"` // Is it walkable?
	speedmod       int  `json:"speedmod"`  // How much to slow ya down or speed ya up
	damageovertime int  `json:"dot"`       // Does it hurt to walk on?
}

type Tiles struct {
	TileType    string `json:"type"`        // Type of tile
	Coordinates Cords  `json:"coordinates"` // Where does this tile start?
	Properties  Props  `json:"properties"`  // Special settings
	Fill        Cords  `json:"fill"`        // Starting from coordinates fill until...
}

// Container for the json
type MapObj struct {
	Map Dimension
	// tiles
}
